use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cli::{
    ServiceCommand, ServiceInstallArgs, ServiceLogArgs, ServiceTargetArgs, ServiceUninstallArgs,
};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

#[derive(Debug, Clone)]
struct CmdCapturedOutput {
    stdout: String,
    stderr: String,
}

pub fn run(command: ServiceCommand) -> Result<()> {
    match command {
        ServiceCommand::Install(args) => cmd_install(args),
        ServiceCommand::Uninstall(args) => cmd_uninstall(args),
        ServiceCommand::Enable(target) => cmd_simple_action(target, "enable"),
        ServiceCommand::Disable(target) => cmd_simple_action(target, "disable"),
        ServiceCommand::Start(target) => cmd_simple_action(target, "start"),
        ServiceCommand::Stop(target) => cmd_simple_action(target, "stop"),
        ServiceCommand::Restart(target) => cmd_simple_action(target, "restart"),
        ServiceCommand::Status(target) => cmd_status(target),
        ServiceCommand::Log(args) => cmd_log(args),
    }
}

fn cmd_install(args: ServiceInstallArgs) -> Result<()> {
    ensure_linux_host()?;
    let paths = app_paths()?;

    let unit_name = normalize_unit_name(&args.target.name);
    let unit_path = resolve_unit_path(&args.target, &unit_name)?;

    if unit_path.exists() && !args.force {
        bail!("unit 已存在: {}，如需覆盖请加 --force", unit_path.display());
    }

    let binary = match args.binary {
        Some(p) => p,
        None => paths.core_current_link.clone(),
    };
    if !binary.exists() {
        bail!(
            "未找到内核二进制: {}，请先执行 `clash core install` 或使用 --binary 指定",
            binary.display()
        );
    }

    let config = match args.config {
        Some(p) => p,
        None => paths.runtime_config_file.clone(),
    };

    let workdir = args.workdir.unwrap_or(paths.runtime_dir);
    fs::create_dir_all(&workdir).context("创建工作目录失败")?;

    let mut created_template = false;
    if !config.exists() {
        if let Some(parent) = config.parent() {
            fs::create_dir_all(parent).context("创建配置目录失败")?;
        }
        fs::write(&config, default_runtime_config()).context("写入默认配置失败")?;
        created_template = true;
    }

    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    let unit_content = build_unit_content(&binary, &config, &workdir, args.target.user, &unit_name);
    fs::write(&unit_path, unit_content)
        .with_context(|| format!("写入 unit 文件失败: {}", unit_path.display()))?;

    run_systemctl_raw(args.target.user, &["daemon-reload".to_string()])?;

    if !is_json_mode() {
        println!("service unit 安装完成: {}", unit_path.display());
        println!("服务名: {}", unit_name);
        println!("工作目录: {}", workdir.display());
        println!("配置文件: {}", config.display());
        println!("内核路径: {}", binary.display());
    }

    let mut enabled = false;
    if !args.no_enable {
        run_systemctl_unit_action(&args.target, "enable")?;
        enabled = true;
        if !is_json_mode() {
            println!("已启用开机自启。");
        }
    }

    let mut started = false;
    if created_template {
        if !is_json_mode() {
            println!("检测到配置不存在，已生成模板配置。");
            println!("请先编辑配置后再启动: {}", config.display());
        }
    } else if !args.no_start {
        run_systemctl_unit_action(&args.target, "start")?;
        started = true;
        if !is_json_mode() {
            println!("服务已启动。");
        }
    }

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "service.install",
            "unit": unit_name,
            "unit_path": unit_path.display().to_string(),
            "workdir": workdir.display().to_string(),
            "config": config.display().to_string(),
            "binary": binary.display().to_string(),
            "enabled": enabled,
            "started": started,
            "template_created": created_template
        }));
    }

    Ok(())
}

fn cmd_uninstall(args: ServiceUninstallArgs) -> Result<()> {
    ensure_linux_host()?;
    let paths = app_paths()?;
    let unit_name = normalize_unit_name(&args.target.name);
    let unit_path = resolve_unit_path(&args.target, &unit_name)?;

    run_systemctl_unit_action_best_effort(&args.target, "stop", "停止服务失败，继续卸载");
    run_systemctl_unit_action_best_effort(&args.target, "disable", "禁用服务失败，继续卸载");
    run_systemctl_unit_action_best_effort(
        &args.target,
        "reset-failed",
        "重置失败状态异常，继续卸载",
    );

    let mut unit_deleted = false;
    if unit_path.exists() {
        fs::remove_file(&unit_path)
            .with_context(|| format!("删除 unit 失败: {}", unit_path.display()))?;
        unit_deleted = true;
        if !is_json_mode() {
            println!("已删除 unit: {}", unit_path.display());
        }
    } else {
        if !is_json_mode() {
            println!("unit 不存在，无需删除: {}", unit_path.display());
        }
    }

    run_systemctl_raw(args.target.user, &["daemon-reload".to_string()])?;
    if !is_json_mode() {
        println!("已完成 systemd daemon-reload。");
    }

    let mut runtime_purged = false;
    if args.purge {
        if paths.runtime_dir.exists() {
            fs::remove_dir_all(&paths.runtime_dir).with_context(|| {
                format!("清理 runtime 目录失败: {}", paths.runtime_dir.display())
            })?;
            runtime_purged = true;
            if !is_json_mode() {
                println!("已清理 runtime 目录: {}", paths.runtime_dir.display());
            }
        } else {
            if !is_json_mode() {
                println!(
                    "runtime 目录不存在，无需清理: {}",
                    paths.runtime_dir.display()
                );
            }
        }
    }

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "service.uninstall",
            "unit": unit_name,
            "unit_path": unit_path.display().to_string(),
            "unit_deleted": unit_deleted,
            "purge_requested": args.purge,
            "runtime_purged": runtime_purged
        }));
    }

    println!("服务卸载完成: {}", unit_name);
    Ok(())
}

fn cmd_simple_action(target: ServiceTargetArgs, action: &str) -> Result<()> {
    ensure_linux_host()?;
    run_systemctl_unit_action(&target, action)?;
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": format!("service.{action}"),
            "unit": normalize_unit_name(&target.name),
            "user": target.user
        }));
    }
    let verb = match action {
        "enable" => "已启用",
        "disable" => "已禁用",
        "start" => "已启动",
        "stop" => "已停止",
        "restart" => "已重启",
        _ => "已执行",
    };
    println!("{} {}", verb, normalize_unit_name(&target.name));
    Ok(())
}

fn cmd_status(target: ServiceTargetArgs) -> Result<()> {
    ensure_linux_host()?;
    let unit = normalize_unit_name(&target.name);

    let mut args = Vec::new();
    args.push("status".to_string());
    args.push(unit);
    args.push("--no-pager".to_string());
    let output = run_systemctl_raw(target.user, &args)?;
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "service.status",
            "unit": normalize_unit_name(&target.name),
            "user": target.user,
            "stdout": output.stdout,
            "stderr": output.stderr
        }));
    }
    Ok(())
}

fn cmd_log(args: ServiceLogArgs) -> Result<()> {
    ensure_linux_host()?;
    let unit = normalize_unit_name(&args.target.name);

    if is_json_mode() {
        if args.follow {
            bail!("--json 模式暂不支持 `service log --follow`");
        }
        let mut cmd = Command::new("journalctl");
        if args.target.user {
            cmd.arg("--user");
        }
        let output = cmd
            .arg("-u")
            .arg(&unit)
            .arg("-n")
            .arg(args.lines.to_string())
            .arg("--no-pager")
            .output()
            .context("执行 journalctl 失败")?;
        if !output.status.success() {
            bail!("journalctl 返回非成功状态: {}", output.status);
        }
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "service.log",
            "unit": unit,
            "user": args.target.user,
            "lines": args.lines,
            "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string()
        }));
    }

    let mut cmd = Command::new("journalctl");
    if args.target.user {
        cmd.arg("--user");
    }
    cmd.arg("-u").arg(unit);
    cmd.arg("-n").arg(args.lines.to_string());
    cmd.arg("--no-pager");
    if args.follow {
        cmd.arg("-f");
    }

    let status = cmd.status().context("执行 journalctl 失败")?;
    if !status.success() {
        bail!("journalctl 返回非成功状态: {}", status);
    }
    Ok(())
}

fn run_systemctl_unit_action(target: &ServiceTargetArgs, action: &str) -> Result<()> {
    let unit = normalize_unit_name(&target.name);
    let args = vec![action.to_string(), unit];
    run_systemctl_raw(target.user, &args).map(|_| ())
}

fn run_systemctl_unit_action_best_effort(target: &ServiceTargetArgs, action: &str, msg: &str) {
    if let Err(err) = run_systemctl_unit_action(target, action) {
        if !is_json_mode() {
            eprintln!("警告: {}: {}", msg, err);
        }
    }
}

fn run_systemctl_raw(user: bool, args: &[String]) -> Result<CmdCapturedOutput> {
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("执行 systemctl 失败")?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !is_json_mode() {
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
    }
    if !output.status.success() {
        bail!(
            "systemctl 返回非成功状态: {} (stdout={}, stderr={})",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(CmdCapturedOutput { stdout, stderr })
}

fn normalize_unit_name(name: &str) -> String {
    if name.ends_with(".service") {
        name.to_string()
    } else {
        format!("{name}.service")
    }
}

fn resolve_unit_path(target: &ServiceTargetArgs, unit_name: &str) -> Result<PathBuf> {
    if target.user {
        let home = dirs::home_dir().context("无法获取 home 目录")?;
        return Ok(home
            .join(".config")
            .join("systemd")
            .join("user")
            .join(unit_name));
    }
    Ok(PathBuf::from("/etc/systemd/system").join(unit_name))
}

fn build_unit_content(
    binary: &Path,
    config: &Path,
    workdir: &Path,
    user_service: bool,
    unit_name: &str,
) -> String {
    let wanted_by = if user_service {
        "default.target"
    } else {
        "multi-user.target"
    };

    format!(
        "[Unit]\n\
         Description=clash-cli managed {unit_name}\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={workdir}\n\
         ExecStart={binary} -d {workdir} -f {config}\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         LimitNOFILE=1048576\n\
         AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW\n\
         CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy={wanted_by}\n",
        binary = binary.display(),
        config = config.display(),
        workdir = workdir.display(),
    )
}

fn default_runtime_config() -> &'static str {
    "mixed-port: 7890\n\
     allow-lan: false\n\
     mode: rule\n\
     log-level: info\n\
     external-controller: 127.0.0.1:9090\n\
     secret: \"\"\n\
     dns:\n\
       enable: true\n\
       enhanced-mode: fake-ip\n"
}

fn ensure_linux_host() -> Result<()> {
    if env::consts::OS != "linux" {
        bail!("当前仅支持 Linux 平台");
    }
    Ok(())
}
