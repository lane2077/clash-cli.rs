//! 桌面「系统代理」：与终端 env、TUN 分开，对标 Clash Verge 的 System Proxy 开关。

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::utils;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemProxyBackend {
    Gnome,
    Macos,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MacosProxyEndpointState {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
    #[serde(default)]
    pub authenticated: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MacosProxyServiceState {
    pub service: String,
    pub web: MacosProxyEndpointState,
    pub secure_web: MacosProxyEndpointState,
    pub socks: MacosProxyEndpointState,
    #[serde(default)]
    pub bypass_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemProxyRecord {
    pub backend: SystemProxyBackend,
    pub enabled: bool,
    pub host: String,
    pub http_port: u16,
    pub socks_port: u16,
    #[serde(default)]
    pub previous_gnome_mode: Option<String>,
    #[serde(default)]
    pub previous_macos: Vec<MacosProxyServiceState>,
}

#[derive(Clone, Debug)]
pub struct SysCmd {
    pub program: String,
    pub args: Vec<String>,
}

impl SysCmd {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

pub fn detect_backend() -> Option<SystemProxyBackend> {
    if cfg!(target_os = "macos") && utils::command_exists("networksetup") {
        return Some(SystemProxyBackend::Macos);
    }
    if utils::command_exists("gsettings") {
        return Some(SystemProxyBackend::Gnome);
    }
    None
}

pub fn gvariant_string_array(csv: &str) -> String {
    let items: Vec<String> = csv
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("'{s}'"))
        .collect();
    format!("[{}]", items.join(", "))
}

pub fn gnome_enable_commands(
    host: &str,
    http_port: u16,
    socks_port: u16,
    no_proxy: &str,
) -> Vec<SysCmd> {
    let http_port = http_port.to_string();
    let socks_port = socks_port.to_string();
    let ignore = gvariant_string_array(no_proxy);
    vec![
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy", "mode", "manual"],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.http", "host", host],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.http", "port", &http_port],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.https", "host", host],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.https", "port", &http_port],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.socks", "host", host],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy.socks", "port", &socks_port],
        ),
        SysCmd::new(
            "gsettings",
            &["set", "org.gnome.system.proxy", "ignore-hosts", &ignore],
        ),
    ]
}

pub fn gnome_disable_commands(previous_mode: Option<&str>) -> Vec<SysCmd> {
    let mode = previous_mode.unwrap_or("none");
    vec![SysCmd::new(
        "gsettings",
        &["set", "org.gnome.system.proxy", "mode", mode],
    )]
}

pub fn parse_gsettings_string(raw: &str) -> String {
    raw.trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim()
        .to_string()
}

pub fn parse_networksetup_services(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("An asterisk"))
        .filter(|l| !l.starts_with('*'))
        .map(|s| s.to_string())
        .collect()
}

pub fn macos_enable_commands(
    services: &[String],
    host: &str,
    http_port: u16,
    socks_port: u16,
    no_proxy: &str,
) -> Vec<SysCmd> {
    let http = http_port.to_string();
    let socks = socks_port.to_string();
    let mut bypass: Vec<&str> = no_proxy
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if bypass.is_empty() {
        bypass = vec!["localhost", "127.0.0.1"];
    }
    let mut cmds = Vec::new();
    for svc in services {
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setwebproxy", svc, host, &http],
        ));
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setsecurewebproxy", svc, host, &http],
        ));
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setsocksfirewallproxy", svc, host, &socks],
        ));
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setwebproxystate", svc, "on"],
        ));
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setsecurewebproxystate", svc, "on"],
        ));
        cmds.push(SysCmd::new(
            "networksetup",
            &["-setsocksfirewallproxystate", svc, "on"],
        ));
        let mut bypass_args = vec!["-setproxybypassdomains", svc.as_str()];
        bypass_args.extend(bypass.iter().copied());
        cmds.push(SysCmd::new("networksetup", &bypass_args));
    }
    cmds
}

fn parse_networksetup_proxy(raw: &str) -> Result<MacosProxyEndpointState> {
    let mut state = MacosProxyEndpointState::default();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "Enabled" => state.enabled = value.eq_ignore_ascii_case("yes"),
            "Server" => state.server = value.to_string(),
            "Port" => state.port = value.parse::<u16>().unwrap_or(0),
            "Authenticated Proxy Enabled" => {
                state.authenticated = value == "1" || value.eq_ignore_ascii_case("yes")
            }
            _ => {}
        }
    }
    Ok(state)
}

fn networksetup_text(args: &[&str]) -> Result<String> {
    let output = Command::new("networksetup")
        .args(args)
        .output()
        .with_context(|| format!("执行 networksetup {} 失败", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "networksetup {} 失败: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn snapshot_macos_services(services: &[String]) -> Result<Vec<MacosProxyServiceState>> {
    let mut result = Vec::with_capacity(services.len());
    for service in services {
        let web = parse_networksetup_proxy(&networksetup_text(&["-getwebproxy", service])?)?;
        let secure_web =
            parse_networksetup_proxy(&networksetup_text(&["-getsecurewebproxy", service])?)?;
        let socks =
            parse_networksetup_proxy(&networksetup_text(&["-getsocksfirewallproxy", service])?)?;
        if web.authenticated || secure_web.authenticated || socks.authenticated {
            bail!("网络服务 `{service}` 已配置认证代理；clash-cli 无法安全保存密码，因此拒绝覆盖");
        }
        let bypass_raw = networksetup_text(&["-getproxybypassdomains", service])?;
        let bypass_domains = bypass_raw
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("There aren't any")
                    && !line.starts_with("There are no")
            })
            .map(str::to_string)
            .collect();
        result.push(MacosProxyServiceState {
            service: service.clone(),
            web,
            secure_web,
            socks,
            bypass_domains,
        });
    }
    Ok(result)
}

fn restore_endpoint_commands(
    commands: &mut Vec<SysCmd>,
    service: &str,
    endpoint: &MacosProxyEndpointState,
    set_command: &str,
    state_command: &str,
) {
    if !endpoint.server.is_empty() && endpoint.port != 0 {
        let port = endpoint.port.to_string();
        commands.push(SysCmd::new(
            "networksetup",
            &[set_command, service, &endpoint.server, &port],
        ));
    }
    commands.push(SysCmd::new(
        "networksetup",
        &[
            state_command,
            service,
            if endpoint.enabled { "on" } else { "off" },
        ],
    ));
}

pub fn macos_disable_commands(services: &[String]) -> Vec<SysCmd> {
    let mut commands = Vec::new();
    for service in services {
        commands.push(SysCmd::new(
            "networksetup",
            &["-setwebproxystate", service, "off"],
        ));
        commands.push(SysCmd::new(
            "networksetup",
            &["-setsecurewebproxystate", service, "off"],
        ));
        commands.push(SysCmd::new(
            "networksetup",
            &["-setsocksfirewallproxystate", service, "off"],
        ));
    }
    commands
}

pub fn macos_restore_commands(states: &[MacosProxyServiceState]) -> Vec<SysCmd> {
    let mut commands = Vec::new();
    for state in states {
        restore_endpoint_commands(
            &mut commands,
            &state.service,
            &state.web,
            "-setwebproxy",
            "-setwebproxystate",
        );
        restore_endpoint_commands(
            &mut commands,
            &state.service,
            &state.secure_web,
            "-setsecurewebproxy",
            "-setsecurewebproxystate",
        );
        restore_endpoint_commands(
            &mut commands,
            &state.service,
            &state.socks,
            "-setsocksfirewallproxy",
            "-setsocksfirewallproxystate",
        );
        let mut args = vec!["-setproxybypassdomains", state.service.as_str()];
        if state.bypass_domains.is_empty() {
            args.push("Empty");
        } else {
            args.extend(state.bypass_domains.iter().map(String::as_str));
        }
        commands.push(SysCmd::new("networksetup", &args));
    }
    commands
}

pub fn run_sys_cmd(cmd: &SysCmd) -> Result<()> {
    let output = Command::new(&cmd.program)
        .args(&cmd.args)
        .output()
        .with_context(|| format!("执行失败: {} {}", cmd.program, cmd.args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{} {} 失败: {}",
            cmd.program,
            cmd.args.join(" "),
            stderr.trim()
        );
    }
    Ok(())
}

pub fn gnome_current_mode() -> Result<String> {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.system.proxy", "mode"])
        .output()
        .context("读取 gsettings 代理模式失败")?;
    if !output.status.success() {
        bail!("读取 gsettings 代理模式失败");
    }
    Ok(parse_gsettings_string(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn list_macos_services() -> Result<Vec<String>> {
    let output = Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .context("执行 networksetup -listallnetworkservices 失败")?;
    if !output.status.success() {
        bail!("列出网络服务失败");
    }
    Ok(parse_networksetup_services(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn read_record(path: &Path) -> Result<Option<SystemProxyRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("读取系统代理状态失败: {}", path.display()))?;
    Ok(Some(
        serde_json::from_str(&text).context("解析系统代理状态失败")?,
    ))
}

pub fn write_record(path: &Path, record: &SystemProxyRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(record).context("序列化系统代理状态失败")?;
    crate::utils::write_atomic_text(path, &text)
        .with_context(|| format!("写入系统代理状态失败: {}", path.display()))
}

pub fn clear_record(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("删除状态失败: {}", path.display()))?;
    }
    Ok(())
}

pub fn unsupported_hint() -> &'static str {
    "未检测到桌面系统代理接口（Linux 需 gsettings / GNOME；macOS 需 networksetup）。\n\
     当前终端: eval \"$(clash env on)\"\n\
     全局接管: clash tun on"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_enable_sets_manual_http_and_socks() {
        let cmds = gnome_enable_commands("127.0.0.1", 7890, 7891, "localhost,127.0.0.1");
        let joined: Vec<String> = cmds
            .iter()
            .map(|c| format!("{} {}", c.program, c.args.join(" ")))
            .collect();
        assert!(joined.iter().any(|s| s.contains("mode manual")));
        assert!(joined.iter().any(|s| s.contains("http host 127.0.0.1")));
        assert!(joined.iter().any(|s| s.contains("http port 7890")));
        assert!(joined.iter().any(|s| s.contains("socks port 7891")));
        assert!(joined.iter().any(|s| s.contains("ignore-hosts")));
    }

    #[test]
    fn gnome_disable_restores_previous_mode() {
        let cmds = gnome_disable_commands(Some("auto"));
        assert_eq!(
            cmds[0].args,
            ["set", "org.gnome.system.proxy", "mode", "auto"]
        );
    }

    #[test]
    fn parse_gsettings_strips_quotes() {
        assert_eq!(parse_gsettings_string("  'manual'\n"), "manual");
    }

    #[test]
    fn parse_networksetup_skips_disabled() {
        let raw = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nEthernet\n";
        assert_eq!(
            parse_networksetup_services(raw),
            vec!["Wi-Fi".to_string(), "Ethernet".to_string()]
        );
    }

    #[test]
    fn gvariant_array_quotes_items() {
        assert_eq!(
            gvariant_string_array("localhost,127.0.0.1"),
            "['localhost', '127.0.0.1']"
        );
    }

    #[test]
    fn parse_networksetup_proxy_state() {
        let state = parse_networksetup_proxy(
            "Enabled: Yes\nServer: proxy.example\nPort: 8080\nAuthenticated Proxy Enabled: 0\n",
        )
        .expect("解析失败");
        assert!(state.enabled);
        assert_eq!(state.server, "proxy.example");
        assert_eq!(state.port, 8080);
        assert!(!state.authenticated);
    }

    #[test]
    fn macos_legacy_disable_turns_managed_proxies_off() {
        let blob = macos_disable_commands(&["Wi-Fi".into()])
            .iter()
            .map(|cmd| cmd.args.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(blob.contains("-setwebproxystate Wi-Fi off"));
        assert!(blob.contains("-setsecurewebproxystate Wi-Fi off"));
        assert!(blob.contains("-setsocksfirewallproxystate Wi-Fi off"));
    }

    #[test]
    fn macos_restore_reinstates_original_endpoint_state() {
        let state = MacosProxyServiceState {
            service: "Wi-Fi".into(),
            web: MacosProxyEndpointState {
                enabled: true,
                server: "old.proxy".into(),
                port: 3128,
                authenticated: false,
            },
            secure_web: MacosProxyEndpointState::default(),
            socks: MacosProxyEndpointState::default(),
            bypass_domains: vec!["localhost".into()],
        };
        let blob = macos_restore_commands(&[state])
            .iter()
            .map(|cmd| cmd.args.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(blob.contains("-setwebproxy Wi-Fi old.proxy 3128"));
        assert!(blob.contains("-setwebproxystate Wi-Fi on"));
        assert!(blob.contains("-setsecurewebproxystate Wi-Fi off"));
        assert!(blob.contains("-setproxybypassdomains Wi-Fi localhost"));
    }

    #[test]
    fn macos_enable_covers_web_and_socks() {
        let cmds = macos_enable_commands(
            &["Wi-Fi".into()],
            "127.0.0.1",
            7890,
            7891,
            "localhost,127.0.0.1",
        );
        let blob = cmds
            .iter()
            .map(|c| c.args.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(blob.contains("-setwebproxy Wi-Fi 127.0.0.1 7890"));
        assert!(blob.contains("-setsocksfirewallproxy Wi-Fi 127.0.0.1 7891"));
        assert!(blob.contains("-setwebproxystate Wi-Fi on"));
    }
}
