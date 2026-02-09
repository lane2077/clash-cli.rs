use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_yaml::Value as YamlValue;

use crate::cli::{AutoAction, EnvAction, ProxyCommand, ShellKind, StartArgs, StopArgs};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

const HOOK_START: &str = "# >>> clash-cli proxy >>>";
const HOOK_END: &str = "# <<< clash-cli proxy <<<";
const DEFAULT_PROXY_HOST: &str = "127.0.0.1";
const DEFAULT_HTTP_PORT: u16 = 7890;
const DEFAULT_SOCKS_PORT: u16 = 7891;
const SHELL_HOOK_BODY: &str = r#"if [ -n "$CLASH_CLI_HOME" ] && [ -f "$CLASH_CLI_HOME/proxy.env" ]; then
  . "$CLASH_CLI_HOME/proxy.env"
elif [ -n "$XDG_CONFIG_HOME" ] && [ -f "$XDG_CONFIG_HOME/clash-cli/proxy.env" ]; then
  . "$XDG_CONFIG_HOME/clash-cli/proxy.env"
elif [ -f "$HOME/.config/clash-cli/proxy.env" ]; then
  . "$HOME/.config/clash-cli/proxy.env"
fi"#;

#[derive(Debug, Clone, Serialize)]
struct ProxyState {
    host: String,
    http_port: u16,
    socks_port: u16,
    no_proxy: String,
}

#[derive(Debug, Clone, Default)]
struct RuntimeProxyDefaults {
    host: Option<String>,
    mixed_port: Option<u16>,
    http_port: Option<u16>,
    socks_port: Option<u16>,
}

impl ProxyState {
    fn to_state_file(&self) -> String {
        format!(
            "host={}\nhttp_port={}\nsocks_port={}\nno_proxy={}\n",
            self.host, self.http_port, self.socks_port, self.no_proxy
        )
    }

    fn from_state_file(content: &str) -> Result<Self> {
        let mut host = None;
        let mut http_port = None;
        let mut socks_port = None;
        let mut no_proxy = None;

        for line in content.lines() {
            let mut parts = line.splitn(2, '=');
            let key = parts.next().unwrap_or_default().trim();
            let value = parts.next().unwrap_or_default().trim();

            match key {
                "host" => host = Some(value.to_string()),
                "http_port" => http_port = Some(value.parse::<u16>().context("http_port 无效")?),
                "socks_port" => socks_port = Some(value.parse::<u16>().context("socks_port 无效")?),
                "no_proxy" => no_proxy = Some(value.to_string()),
                _ => {}
            }
        }

        Ok(Self {
            host: host.context("状态文件缺少 host")?,
            http_port: http_port.context("状态文件缺少 http_port")?,
            socks_port: socks_port.context("状态文件缺少 socks_port")?,
            no_proxy: no_proxy.context("状态文件缺少 no_proxy")?,
        })
    }

    fn export_script(&self) -> String {
        let http_endpoint = format!("http://{}:{}", self.host, self.http_port);
        let socks_endpoint = format!("socks5://{}:{}", self.host, self.socks_port);
        format!(
            "export http_proxy={http}\n\
             export https_proxy={http}\n\
             export HTTP_PROXY={http}\n\
             export HTTPS_PROXY={http}\n\
             export all_proxy={socks}\n\
             export ALL_PROXY={socks}\n\
             export no_proxy={no_proxy}\n\
             export NO_PROXY={no_proxy}\n",
            http = http_endpoint,
            socks = socks_endpoint,
            no_proxy = self.no_proxy
        )
    }
}

pub fn run(command: ProxyCommand) -> Result<()> {
    match command {
        ProxyCommand::Start(args) => cmd_start(args),
        ProxyCommand::Stop(args) => cmd_stop(args),
        ProxyCommand::Status => cmd_status(),
        ProxyCommand::Env { action } => cmd_env(action),
        ProxyCommand::Auto { action } => cmd_auto(action),
    }
}

fn cmd_start(args: StartArgs) -> Result<()> {
    let paths = app_paths()?;
    let runtime_defaults = load_runtime_proxy_defaults(&paths.runtime_config_file);
    let state = resolve_start_proxy_state(&args, runtime_defaults);

    fs::create_dir_all(&paths.config_dir).context("创建配置目录失败")?;
    fs::write(&paths.state_file, state.to_state_file()).context("写入代理状态失败")?;
    fs::write(&paths.env_file, state.export_script()).context("写入代理环境文件失败")?;

    let mut auto_shell = None;
    if args.auto {
        let shell = args.shell.unwrap_or(detect_shell()?);
        install_shell_hook(shell)?;
        auto_shell = Some(shell);
    }

    let export_script = state.export_script();
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "proxy.start",
            "state": state,
            "auto": args.auto,
            "auto_shell": auto_shell.map(|v| v.as_str().to_string()),
            "script": if args.print_env { Some(export_script) } else { None },
            "hint": "eval \"$(clash proxy env on)\""
        }));
    }

    if args.print_env || !io::stdout().is_terminal() {
        print!("{}", export_script);
        return Ok(());
    }

    println!(
        "已保存代理配置: http://{}:{}, socks5://{}:{}",
        state.host, state.http_port, state.host, state.socks_port
    );
    println!("在当前终端生效:");
    println!("  eval \"$(clash proxy env on)\"");
    if args.auto {
        println!("已开启新终端自动启用代理。");
    } else {
        println!("如需新终端自动启用:");
        println!("  clash proxy auto on --shell {}", detect_shell()?.as_str());
    }
    Ok(())
}

fn resolve_start_proxy_state(args: &StartArgs, runtime: RuntimeProxyDefaults) -> ProxyState {
    let host = args
        .host
        .clone()
        .or(runtime.host)
        .unwrap_or_else(|| DEFAULT_PROXY_HOST.to_string());

    let http_port = args
        .http_port
        .or(runtime.mixed_port)
        .or(runtime.http_port)
        .unwrap_or(DEFAULT_HTTP_PORT);

    let socks_port = args
        .socks_port
        .or(runtime.socks_port)
        .or(runtime.mixed_port)
        .unwrap_or(DEFAULT_SOCKS_PORT);

    ProxyState {
        host,
        http_port,
        socks_port,
        no_proxy: args.no_proxy.clone(),
    }
}

fn load_runtime_proxy_defaults(path: &Path) -> RuntimeProxyDefaults {
    match try_load_runtime_proxy_defaults(path) {
        Ok(v) => v,
        Err(err) => {
            if !is_json_mode() {
                eprintln!("警告: 读取 runtime 配置失败，将回退默认代理端口: {}", err);
            }
            RuntimeProxyDefaults::default()
        }
    }
}

fn try_load_runtime_proxy_defaults(path: &Path) -> Result<RuntimeProxyDefaults> {
    if !path.exists() {
        return Ok(RuntimeProxyDefaults::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 runtime 配置失败: {}", path.display()))?;
    let root: YamlValue = serde_yaml::from_str(&content)
        .with_context(|| format!("解析 runtime 配置失败: {}", path.display()))?;

    let host = yaml_root_string(&root, "bind-address").and_then(|v| normalize_runtime_host(&v));
    let mixed_port = yaml_root_u16(&root, "mixed-port");
    let http_port = yaml_root_u16(&root, "port");
    let socks_port = yaml_root_u16(&root, "socks-port");

    Ok(RuntimeProxyDefaults {
        host,
        mixed_port,
        http_port,
        socks_port,
    })
}

fn yaml_root_string(root: &YamlValue, key: &str) -> Option<String> {
    root.as_mapping()
        .and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
}

fn yaml_root_u16(root: &YamlValue, key: &str) -> Option<u16> {
    root.as_mapping()
        .and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(|v| {
            if let Some(i) = v.as_i64() {
                return u16::try_from(i).ok();
            }
            if let Some(s) = v.as_str() {
                return s.trim().parse::<u16>().ok();
            }
            None
        })
}

fn normalize_runtime_host(value: &str) -> Option<String> {
    let host = value.trim();
    if host.is_empty() {
        return None;
    }
    if host == "0.0.0.0" || host == "::" || host == "*" {
        return Some(DEFAULT_PROXY_HOST.to_string());
    }
    Some(host.to_string())
}

fn cmd_stop(args: StopArgs) -> Result<()> {
    let paths = app_paths()?;
    if paths.state_file.exists() {
        fs::remove_file(&paths.state_file).context("删除代理状态失败")?;
    }
    if paths.env_file.exists() {
        fs::remove_file(&paths.env_file).context("删除代理环境文件失败")?;
    }

    let mut auto_shell = None;
    if args.auto_off {
        let shell = args.shell.unwrap_or(detect_shell()?);
        uninstall_shell_hook(shell)?;
        auto_shell = Some(shell);
    }

    let script = unset_script();
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "proxy.stop",
            "auto_off": args.auto_off,
            "auto_shell": auto_shell.map(|v| v.as_str().to_string()),
            "script": if args.print_env { Some(script) } else { None },
            "hint": "eval \"$(clash proxy env off)\""
        }));
    }

    if args.print_env || !io::stdout().is_terminal() {
        print!("{}", script);
        return Ok(());
    }

    println!("已清理代理配置。");
    println!("在当前终端关闭:");
    println!("  eval \"$(clash proxy env off)\"");
    if args.auto_off {
        println!("已移除自动启用钩子。");
    }
    Ok(())
}

fn cmd_status() -> Result<()> {
    let paths = app_paths()?;
    if !paths.state_file.exists() {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "proxy.status",
                "configured": false,
                "hint": "先执行 `clash proxy start`"
            }));
        }
        println!("代理状态: 未配置");
        println!("提示: 先执行 `clash proxy start`");
        return Ok(());
    }

    let state = load_state(&paths.state_file)?;
    let zsh_auto = shell_hook_installed(ShellKind::Zsh)?;
    let bash_auto = shell_hook_installed(ShellKind::Bash)?;
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "proxy.status",
            "configured": true,
            "state": state,
            "auto": {
                "zsh": zsh_auto,
                "bash": bash_auto
            },
            "hint": "eval \"$(clash proxy env on)\""
        }));
    }

    println!("代理状态: 已配置");
    println!("HTTP/HTTPS: http://{}:{}", state.host, state.http_port);
    println!("SOCKS5: socks5://{}:{}", state.host, state.socks_port);
    println!("NO_PROXY: {}", state.no_proxy);
    println!("当前终端生效命令: eval \"$(clash proxy env on)\"");

    println!("自动启用(zsh): {}", if zsh_auto { "开启" } else { "关闭" });
    println!(
        "自动启用(bash): {}",
        if bash_auto { "开启" } else { "关闭" }
    );

    Ok(())
}

fn cmd_env(action: EnvAction) -> Result<()> {
    match action {
        EnvAction::On => {
            let paths = app_paths()?;
            let state = load_state(&paths.state_file)?;
            let script = state.export_script();
            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "proxy.env.on",
                    "script": script
                }));
            }
            print!("{}", script);
        }
        EnvAction::Off => {
            let script = unset_script();
            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "proxy.env.off",
                    "script": script
                }));
            }
            print!("{}", script);
        }
    }
    Ok(())
}

fn cmd_auto(action: AutoAction) -> Result<()> {
    match action {
        AutoAction::On { .. } => {
            let shell = action.shell().unwrap_or(detect_shell()?);
            install_shell_hook(shell)?;
            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "proxy.auto.on",
                    "shell": shell.as_str(),
                    "enabled": true
                }));
            }
            println!("已为 {} 安装自动启用钩子。", shell.as_str());
        }
        AutoAction::Off { .. } => {
            let shell = action.shell().unwrap_or(detect_shell()?);
            uninstall_shell_hook(shell)?;
            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "proxy.auto.off",
                    "shell": shell.as_str(),
                    "enabled": false
                }));
            }
            println!("已为 {} 移除自动启用钩子。", shell.as_str());
        }
        AutoAction::Status { .. } => {
            if let Some(shell) = action.shell() {
                let enabled = shell_hook_installed(shell)?;
                if is_json_mode() {
                    return print_json(&serde_json::json!({
                        "ok": true,
                        "action": "proxy.auto.status",
                        "shells": {
                            shell.as_str(): enabled
                        }
                    }));
                }
                println!(
                    "{}: {}",
                    shell.as_str(),
                    if enabled { "已开启" } else { "未开启" }
                );
            } else {
                let zsh = shell_hook_installed(ShellKind::Zsh)?;
                let bash = shell_hook_installed(ShellKind::Bash)?;
                if is_json_mode() {
                    return print_json(&serde_json::json!({
                        "ok": true,
                        "action": "proxy.auto.status",
                        "shells": {
                            "zsh": zsh,
                            "bash": bash
                        }
                    }));
                }
                for shell in [ShellKind::Zsh, ShellKind::Bash] {
                    println!(
                        "{}: {}",
                        shell.as_str(),
                        if match shell {
                            ShellKind::Zsh => zsh,
                            ShellKind::Bash => bash,
                        } {
                            "已开启"
                        } else {
                            "未开启"
                        }
                    );
                }
            }
        }
    }
    Ok(())
}

fn load_state(path: &Path) -> Result<ProxyState> {
    if !path.exists() {
        bail!("未找到代理状态，请先执行 `clash proxy start`");
    }
    let content = fs::read_to_string(path).context("读取代理状态失败")?;
    ProxyState::from_state_file(&content)
}

fn unset_script() -> String {
    "unset http_proxy\n\
     unset https_proxy\n\
     unset HTTP_PROXY\n\
     unset HTTPS_PROXY\n\
     unset all_proxy\n\
     unset ALL_PROXY\n\
     unset no_proxy\n\
     unset NO_PROXY\n"
        .to_string()
}

fn shell_hook_block() -> String {
    format!("{HOOK_START}\n{SHELL_HOOK_BODY}\n{HOOK_END}\n")
}

fn shell_rc_path(shell: ShellKind) -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().context("无法获取 home 目录")?;
    let file = match shell {
        ShellKind::Bash => ".bashrc",
        ShellKind::Zsh => ".zshrc",
    };
    Ok(home.join(file))
}

fn install_shell_hook(shell: ShellKind) -> Result<()> {
    let rc_path = shell_rc_path(shell)?;
    let existing = if rc_path.exists() {
        fs::read_to_string(&rc_path).with_context(|| format!("读取 {} 失败", rc_path.display()))?
    } else {
        String::new()
    };
    if existing.contains(HOOK_START) && existing.contains(HOOK_END) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.ends_with('\n') && !updated.is_empty() {
        updated.push('\n');
    }
    updated.push_str(&shell_hook_block());
    fs::write(&rc_path, updated).with_context(|| format!("写入 {} 失败", rc_path.display()))?;
    Ok(())
}

fn uninstall_shell_hook(shell: ShellKind) -> Result<()> {
    let rc_path = shell_rc_path(shell)?;
    if !rc_path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(&rc_path).with_context(|| format!("读取 {} 失败", rc_path.display()))?;
    let updated = remove_hook_block(&content);
    fs::write(&rc_path, updated).with_context(|| format!("写入 {} 失败", rc_path.display()))?;
    Ok(())
}

fn remove_hook_block(content: &str) -> String {
    let mut output = Vec::new();
    let mut inside_block = false;
    for line in content.lines() {
        if line.trim() == HOOK_START {
            inside_block = true;
            continue;
        }
        if line.trim() == HOOK_END {
            inside_block = false;
            continue;
        }
        if !inside_block {
            output.push(line);
        }
    }
    if output.is_empty() {
        String::new()
    } else {
        format!("{}\n", output.join("\n"))
    }
}

fn shell_hook_installed(shell: ShellKind) -> Result<bool> {
    let rc_path = shell_rc_path(shell)?;
    if !rc_path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(rc_path).context("读取 shell 配置失败")?;
    Ok(content.contains(HOOK_START) && content.contains(HOOK_END))
}

fn detect_shell() -> Result<ShellKind> {
    let shell = env::var("SHELL").unwrap_or_default();
    if shell.ends_with("zsh") {
        return Ok(ShellKind::Zsh);
    }
    if shell.ends_with("bash") {
        return Ok(ShellKind::Bash);
    }
    bail!("无法识别 shell，请用 `--shell bash|zsh` 指定")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_start_proxy_state_should_prefer_runtime_mixed_port() {
        let args = StartArgs {
            host: None,
            http_port: None,
            socks_port: None,
            no_proxy: "localhost".to_string(),
            auto: false,
            shell: None,
            print_env: false,
        };
        let runtime = RuntimeProxyDefaults {
            host: Some("127.0.0.1".to_string()),
            mixed_port: Some(9981),
            http_port: None,
            socks_port: None,
        };
        let state = resolve_start_proxy_state(&args, runtime);
        assert_eq!(state.host, "127.0.0.1");
        assert_eq!(state.http_port, 9981);
        assert_eq!(state.socks_port, 9981);
    }

    #[test]
    fn resolve_start_proxy_state_should_prefer_explicit_args() {
        let args = StartArgs {
            host: Some("127.0.0.1".to_string()),
            http_port: Some(7890),
            socks_port: Some(7891),
            no_proxy: "localhost".to_string(),
            auto: false,
            shell: None,
            print_env: false,
        };
        let runtime = RuntimeProxyDefaults {
            host: Some("192.168.1.2".to_string()),
            mixed_port: Some(9981),
            http_port: Some(9990),
            socks_port: Some(9991),
        };
        let state = resolve_start_proxy_state(&args, runtime);
        assert_eq!(state.host, "127.0.0.1");
        assert_eq!(state.http_port, 7890);
        assert_eq!(state.socks_port, 7891);
    }

    #[test]
    fn normalize_runtime_host_should_replace_any_bind() {
        assert_eq!(
            normalize_runtime_host("0.0.0.0").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(normalize_runtime_host("::").as_deref(), Some("127.0.0.1"));
        assert_eq!(normalize_runtime_host("*").as_deref(), Some("127.0.0.1"));
    }
}
