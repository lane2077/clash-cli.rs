use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::cli::{
    ServiceCommand, ServiceInstallArgs, ServiceLogArgs, ServiceTargetArgs, ServiceUninstallArgs,
};
use crate::constants;
use crate::machine::{ErrorCode, coded_error};
use crate::output::{is_machine_mode, print_machine};
use crate::paths::app_paths;
use crate::utils;

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
    utils::ensure_supported_host()?;
    if utils::is_macos() {
        return darwin_cmd_install(args);
    }
    let paths = app_paths()?;

    let unit_name = utils::normalize_unit_name(&args.target.name);
    let unit_path = resolve_unit_path(&args.target, &unit_name)?;

    if unit_path.exists() && !args.force {
        bail!("unit 已存在: {}，如需覆盖请加 --force", unit_path.display());
    }

    let binary = resolve_service_binary(args.binary.as_deref(), &paths.core_current_link);
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
        utils::write_atomic_text(&config, &default_runtime_config()).context("写入默认配置失败")?;
        created_template = true;
    }

    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    let unit_content = build_unit_content(&binary, &config, &workdir, args.target.user, &unit_name);
    utils::write_atomic_text(&unit_path, &unit_content)
        .with_context(|| format!("写入 unit 文件失败: {}", unit_path.display()))?;

    run_systemctl_raw(args.target.user, &["daemon-reload".to_string()])?;

    if !is_machine_mode() {
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
        if !is_machine_mode() {
            println!("已启用开机自启。");
        }
    }

    let mut started = false;
    if created_template {
        if !is_machine_mode() {
            println!("检测到配置不存在，已生成模板配置。");
            println!("请先编辑配置后再启动: {}", config.display());
        }
    } else if !args.no_start {
        run_systemctl_unit_action(&args.target, "start")?;
        started = true;
        if !is_machine_mode() {
            println!("服务已启动。");
        }
    }

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
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
    utils::ensure_supported_host()?;
    if utils::is_macos() {
        return darwin_cmd_uninstall(args);
    }
    let paths = app_paths()?;
    let unit_name = utils::normalize_unit_name(&args.target.name);
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
        if !is_machine_mode() {
            println!("已删除 unit: {}", unit_path.display());
        }
    } else if !is_machine_mode() {
        println!("unit 不存在，无需删除: {}", unit_path.display());
    }

    run_systemctl_raw(args.target.user, &["daemon-reload".to_string()])?;
    if !is_machine_mode() {
        println!("已完成 systemd daemon-reload。");
    }

    let mut runtime_purged = false;
    if args.purge {
        if paths.runtime_dir.exists() {
            fs::remove_dir_all(&paths.runtime_dir).with_context(|| {
                format!("清理 runtime 目录失败: {}", paths.runtime_dir.display())
            })?;
            runtime_purged = true;
            if !is_machine_mode() {
                println!("已清理 runtime 目录: {}", paths.runtime_dir.display());
            }
        } else if !is_machine_mode() {
            println!(
                "runtime 目录不存在，无需清理: {}",
                paths.runtime_dir.display()
            );
        }
    }

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
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
    utils::ensure_supported_host()?;
    if utils::is_macos() {
        return darwin_cmd_simple_action(target, action);
    }
    run_systemctl_unit_action(&target, action)?;
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": utils::normalize_unit_name(&target.name),
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
    println!("{} {}", verb, utils::normalize_unit_name(&target.name));
    Ok(())
}

fn cmd_status(target: ServiceTargetArgs) -> Result<()> {
    utils::ensure_supported_host()?;
    if utils::is_macos() {
        return darwin_cmd_status(target);
    }
    let unit = utils::normalize_unit_name(&target.name);

    let args = vec!["status".to_string(), unit, "--no-pager".to_string()];
    let output = run_systemctl_raw(target.user, &args)?;
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": utils::normalize_unit_name(&target.name),
            "user": target.user,
            "stdout": output.stdout,
            "stderr": output.stderr
        }));
    }
    Ok(())
}

fn cmd_log(args: ServiceLogArgs) -> Result<()> {
    utils::ensure_supported_host()?;
    if utils::is_macos() {
        return darwin_cmd_log(args);
    }
    let unit = utils::normalize_unit_name(&args.target.name);

    if is_machine_mode() {
        if args.follow {
            bail!("--machine 模式不支持 `service log --follow`");
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
        return print_machine(&serde_json::json!({
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
    let unit = utils::normalize_unit_name(&target.name);
    let args = vec![action.to_string(), unit];
    run_systemctl_raw(target.user, &args).map(|_| ())
}

fn run_systemctl_unit_action_best_effort(target: &ServiceTargetArgs, action: &str, msg: &str) {
    if let Err(err) = run_systemctl_unit_action(target, action)
        && !is_machine_mode()
    {
        eprintln!("警告: {}: {}", msg, err);
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

    let output = cmd.output().map_err(|err| {
        coded_error(
            ErrorCode::ServiceOperationFailed,
            format!("执行 systemctl 失败: {err}"),
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !is_machine_mode() {
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
    }
    if !output.status.success() {
        return Err(coded_error(
            ErrorCode::ServiceOperationFailed,
            format!(
                "systemctl 返回非成功状态: {} (stdout={}, stderr={})",
                output.status,
                stdout.trim(),
                stderr.trim()
            ),
        ));
    }
    Ok(CmdCapturedOutput { stdout, stderr })
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

pub fn restart_managed_service(name: &str, user: bool) -> Result<()> {
    let target = ServiceTargetArgs {
        name: name.to_string(),
        user,
    };
    if utils::is_macos() {
        darwin_run_simple_action(&target, "restart")
    } else {
        utils::ensure_supported_host()?;
        run_systemctl_unit_action(&target, "restart")
    }
}

pub fn is_managed_service_active(name: &str, user: bool) -> Result<bool> {
    if utils::is_macos() {
        return Ok(darwin_service_running(
            &launchd_label(name),
            darwin_use_user_agent(user),
        ));
    }
    if !utils::command_exists("systemctl") {
        bail!("未检测到 systemctl");
    }
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    let unit = utils::normalize_unit_name(name);
    Ok(cmd
        .arg("is-active")
        .arg("--quiet")
        .arg(unit)
        .status()?
        .success())
}

pub fn launchd_label(name: &str) -> String {
    let base = name.strip_suffix(".service").unwrap_or(name);
    format!("com.clash-cli.{base}")
}

pub fn launchd_plist_search_paths(label: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = darwin_plist_path(label, true) {
        paths.push(p);
    }
    if let Ok(p) = darwin_plist_path(label, false) {
        paths.push(p);
    }
    paths
}

/// 从 launchd plist 的 ProgramArguments 取出 `-f` 后的配置路径。
pub fn launchd_plist_config_path(plist: &str) -> Option<PathBuf> {
    let mut strings = Vec::new();
    let mut rest = plist;
    while let Some(start) = rest.find("<string>") {
        rest = &rest[start + 8..];
        let Some(end) = rest.find("</string>") else {
            break;
        };
        strings.push(xml_unescape(&rest[..end]));
        rest = &rest[end + 9..];
    }
    let mut prev_is_f = false;
    for s in strings {
        if prev_is_f {
            return Some(PathBuf::from(s));
        }
        prev_is_f = s == "-f";
    }
    None
}

pub fn build_launchd_plist(binary: &Path, config: &Path, workdir: &Path, label: &str) -> String {
    let bin = xml_escape(&binary.display().to_string());
    let cfg = xml_escape(&config.display().to_string());
    let wd = xml_escape(&workdir.display().to_string());
    let label = xml_escape(label);
    let stdout_log = xml_escape(&workdir.join("mihomo.stdout.log").display().to_string());
    let stderr_log = xml_escape(&workdir.join("mihomo.stderr.log").display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>-d</string>
    <string>{wd}</string>
    <string>-f</string>
    <string>{cfg}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>{wd}</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout_log}</string>
  <key>StandardErrorPath</key>
  <string>{stderr_log}</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn darwin_use_user_agent(user_flag: bool) -> bool {
    user_flag || !utils::is_root_user()
}

fn darwin_plist_path(label: &str, user_agent: bool) -> Result<PathBuf> {
    if user_agent {
        let home = dirs::home_dir().context("无法获取 home 目录")?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{label}.plist")))
    } else {
        Ok(PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist")))
    }
}

fn darwin_domain(user_agent: bool) -> Result<String> {
    if user_agent {
        let uid = std::process::Command::new("id")
            .arg("-u")
            .output()
            .context("读取 uid 失败")?;
        let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
        Ok(format!("gui/{uid}"))
    } else {
        Ok("system".to_string())
    }
}

fn darwin_target(label: &str, user_agent: bool) -> Result<String> {
    Ok(format!("{}/{}", darwin_domain(user_agent)?, label))
}

fn darwin_service_print(label: &str, user_agent: bool) -> Option<CmdCapturedOutput> {
    let target = darwin_target(label, user_agent).ok()?;
    let output = Command::new("launchctl")
        .args(["print", &target])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(CmdCapturedOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn darwin_service_loaded(label: &str, user_agent: bool) -> bool {
    darwin_service_print(label, user_agent).is_some()
}

fn darwin_service_running(label: &str, user_agent: bool) -> bool {
    darwin_service_print(label, user_agent)
        .map(|out| {
            out.stdout
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("state = running"))
        })
        .unwrap_or(false)
}

fn launchctl(args: &[&str]) -> Result<CmdCapturedOutput> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|err| {
            coded_error(
                ErrorCode::ServiceOperationFailed,
                format!("执行 launchctl 失败: {err}"),
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !is_machine_mode() {
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
    }
    if !output.status.success() {
        return Err(coded_error(
            ErrorCode::ServiceOperationFailed,
            format!(
                "launchctl 失败: {} (stdout={}, stderr={})",
                output.status,
                stdout.trim(),
                stderr.trim()
            ),
        ));
    }
    Ok(CmdCapturedOutput { stdout, stderr })
}

fn darwin_cmd_install(args: ServiceInstallArgs) -> Result<()> {
    let paths = app_paths()?;
    let label = launchd_label(&args.target.name);
    let user_agent = darwin_use_user_agent(args.target.user);
    let plist_path = darwin_plist_path(&label, user_agent)?;

    if plist_path.exists() && !args.force {
        bail!(
            "plist 已存在: {}，如需覆盖请加 --force",
            plist_path.display()
        );
    }

    let binary = resolve_service_binary(args.binary.as_deref(), &paths.core_current_link);
    if !binary.exists() {
        bail!(
            "未找到内核二进制: {}，请先执行 `clash core install` 或使用 --binary 指定",
            binary.display()
        );
    }

    let config = args.config.unwrap_or(paths.runtime_config_file.clone());
    let workdir = args.workdir.unwrap_or(paths.runtime_dir.clone());
    fs::create_dir_all(&workdir).context("创建工作目录失败")?;

    let mut created_template = false;
    if !config.exists() {
        if let Some(parent) = config.parent() {
            fs::create_dir_all(parent).context("创建配置目录失败")?;
        }
        utils::write_atomic_text(&config, &default_runtime_config()).context("写入默认配置失败")?;
        created_template = true;
    }

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    let plist = build_launchd_plist(&binary, &config, &workdir, &label);
    utils::write_atomic_text(&plist_path, &plist)
        .with_context(|| format!("写入 plist 失败: {}", plist_path.display()))?;

    let domain = darwin_domain(user_agent)?;
    let target = darwin_target(&label, user_agent)?;
    let _ = Command::new("launchctl")
        .args(["bootout", &target])
        .output();

    let mut enabled = false;
    let mut started = false;
    if !args.no_enable {
        launchctl(&["bootstrap", &domain, &plist_path.display().to_string()])?;
        enabled = true;
        if !created_template && !args.no_start {
            launchctl(&["kickstart", "-k", &target])?;
            started = true;
        }
    }

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": label,
            "unit_path": plist_path.display().to_string(),
            "workdir": workdir.display().to_string(),
            "config": config.display().to_string(),
            "binary": binary.display().to_string(),
            "enabled": enabled,
            "started": started,
            "template_created": created_template,
            "backend": "launchd"
        }));
    }

    println!("launchd 服务已安装: {}", plist_path.display());
    println!("标签: {}", label);
    println!("工作目录: {}", workdir.display());
    println!("配置文件: {}", config.display());
    println!("内核路径: {}", binary.display());
    if created_template {
        println!("已生成模板配置，请先编辑后再启动。");
    }
    Ok(())
}

fn darwin_cmd_uninstall(args: ServiceUninstallArgs) -> Result<()> {
    let paths = app_paths()?;
    let label = launchd_label(&args.target.name);
    let user_agent = darwin_use_user_agent(args.target.user);
    let plist_path = darwin_plist_path(&label, user_agent)?;
    if let Ok(target) = darwin_target(&label, user_agent) {
        let _ = Command::new("launchctl")
            .args(["bootout", &target])
            .output();
    }
    let mut deleted = false;
    if plist_path.exists() {
        fs::remove_file(&plist_path)
            .with_context(|| format!("删除 plist 失败: {}", plist_path.display()))?;
        deleted = true;
    }
    let mut runtime_purged = false;
    if args.purge && paths.runtime_dir.exists() {
        fs::remove_dir_all(&paths.runtime_dir).context("清理 runtime 目录失败")?;
        runtime_purged = true;
    }
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": label,
            "unit_path": plist_path.display().to_string(),
            "unit_deleted": deleted,
            "purge_requested": args.purge,
            "runtime_purged": runtime_purged,
            "backend": "launchd"
        }));
    }
    println!("服务卸载完成: {}", label);
    Ok(())
}

fn darwin_run_simple_action(target: &ServiceTargetArgs, action: &str) -> Result<()> {
    let label = launchd_label(&target.name);
    let user_agent = darwin_use_user_agent(target.user);
    let plist_path = darwin_plist_path(&label, user_agent)?;
    let domain = darwin_domain(user_agent)?;
    let tgt = darwin_target(&label, user_agent)?;
    match action {
        "start" | "enable" => {
            if !darwin_service_loaded(&label, user_agent) {
                if !plist_path.exists() {
                    bail!("未安装服务 plist: {}", plist_path.display());
                }
                launchctl(&["bootstrap", &domain, &plist_path.display().to_string()])?;
            }
            if action == "start" {
                launchctl(&["kickstart", "-k", &tgt])?;
            }
        }
        "stop" | "disable" => {
            if darwin_service_loaded(&label, user_agent) {
                launchctl(&["bootout", &tgt])?;
            }
        }
        "restart" => {
            if darwin_service_loaded(&label, user_agent) {
                launchctl(&["kickstart", "-k", &tgt])?;
            } else if plist_path.exists() {
                launchctl(&["bootstrap", &domain, &plist_path.display().to_string()])?;
                launchctl(&["kickstart", "-k", &tgt])?;
            } else {
                bail!("未安装服务 plist: {}", plist_path.display());
            }
        }
        _ => bail!("不支持的 launchd 动作: {action}"),
    }
    Ok(())
}

fn darwin_cmd_simple_action(target: ServiceTargetArgs, action: &str) -> Result<()> {
    darwin_run_simple_action(&target, action)?;
    let label = launchd_label(&target.name);
    let user_agent = darwin_use_user_agent(target.user);
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": label,
            "user": user_agent,
            "backend": "launchd"
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
    println!("{} {}", verb, label);
    Ok(())
}

fn darwin_cmd_status(target: ServiceTargetArgs) -> Result<()> {
    let label = launchd_label(&target.name);
    let user_agent = darwin_use_user_agent(target.user);
    let status = darwin_service_print(&label, user_agent);
    let running = status
        .as_ref()
        .map(|out| {
            out.stdout
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("state = running"))
        })
        .unwrap_or(false);
    let stdout = status.map(|out| out.stdout).unwrap_or_default();
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "unit": label,
            "user": user_agent,
            "running": running,
            "stdout": stdout,
            "stderr": "",
            "backend": "launchd"
        }));
    }
    if running {
        println!("服务运行中: {}", label);
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
    } else {
        println!("服务未运行: {}", label);
        println!("启动: clash service start --name {}", target.name);
    }
    Ok(())
}

fn darwin_cmd_log(args: ServiceLogArgs) -> Result<()> {
    let paths = app_paths()?;
    let workdir = paths.runtime_dir;
    let log_path = workdir.join("mihomo.stderr.log");
    if is_machine_mode() {
        if args.follow {
            bail!("--machine 模式不支持 `service log --follow`");
        }
        let stdout = if log_path.exists() {
            fs::read_to_string(&log_path).unwrap_or_default()
        } else {
            String::new()
        };
        return print_machine(&serde_json::json!({
            "unit": launchd_label(&args.target.name),
            "path": log_path.display().to_string(),
            "stdout": stdout,
            "stderr": ""
        }));
    }
    if args.follow {
        let status = Command::new("tail")
            .args(["-n", &args.lines.to_string(), "-f"])
            .arg(&log_path)
            .status()
            .context("执行 tail 失败")?;
        if !status.success() {
            bail!("tail 失败: {status}");
        }
        return Ok(());
    }
    if log_path.exists() {
        let content = fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(args.lines);
        for line in &lines[start..] {
            println!("{line}");
        }
    } else {
        println!("暂无日志: {}", log_path.display());
    }
    Ok(())
}

pub fn resolve_service_binary(explicit: Option<&Path>, core_current_link: &Path) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .unwrap_or_else(|| core_current_link.to_path_buf())
}

pub fn build_unit_content(
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
    // systemd --user 不能授予 AmbientCapabilities；把这些字段写进用户
    // unit 会导致服务以 218/CAPABILITIES 退出。需要 TUN 的实例应安装为
    // 系统级服务，普通用户级代理则不需要这些能力。
    let capabilities = if user_service {
        ""
    } else {
        // 系统服务以 root 身份运行，但 CapabilityBoundingSet 会同时裁掉
        // root 原本拥有的文件访问能力。配置通常由普通用户生成，目录和
        // 文件可能是 0700/0600，因此还需要 CAP_DAC_OVERRIDE 才能读取
        // 配置并更新 Mihomo 的运行时缓存。
        "AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_DAC_OVERRIDE\n\
         CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_DAC_OVERRIDE\n\
         NoNewPrivileges=true\n"
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
         {capabilities}\
         \n\
         [Install]\n\
         WantedBy={wanted_by}\n",
        binary = binary.display(),
        config = config.display(),
        workdir = workdir.display(),
    )
}

fn default_runtime_config() -> String {
    format!(
        "mixed-port: {}\n\
         allow-lan: false\n\
         mode: rule\n\
         log-level: info\n\
         external-controller: {}\n\
         secret: \"\"\n\
         dns:\n\
           enable: true\n\
           enhanced-mode: fake-ip\n",
        constants::DEFAULT_MIXED_PORT,
        constants::DEFAULT_CONTROLLER,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_execstart_uses_core_current_link_not_usr_local_copy() {
        let core_current = PathBuf::from("/etc/clash-cli/core/mihomo");
        let binary = resolve_service_binary(None, &core_current);
        assert_eq!(binary, core_current);

        let unit = build_unit_content(
            &binary,
            Path::new("/etc/clash-cli/runtime/config.yaml"),
            Path::new("/etc/clash-cli/runtime"),
            false,
            "clash-mihomo.service",
        );
        assert!(
            unit.contains("ExecStart=/etc/clash-cli/core/mihomo "),
            "ExecStart 应指向 core 软链: {unit}"
        );
        assert!(
            !unit.contains("/usr/local/bin/mihomo"),
            "默认 unit 不应再拷贝到 /usr/local/bin/mihomo: {unit}"
        );
    }

    #[test]
    fn explicit_binary_is_used_when_provided() {
        let core_current = PathBuf::from("/etc/clash-cli/core/mihomo");
        let copy = PathBuf::from("/usr/local/bin/mihomo");
        let binary = resolve_service_binary(Some(copy.as_path()), &core_current);
        assert_eq!(binary, copy);
        let unit = build_unit_content(
            &binary,
            Path::new("/etc/clash-cli/runtime/config.yaml"),
            Path::new("/etc/clash-cli/runtime"),
            false,
            "clash-mihomo.service",
        );
        assert!(unit.contains("ExecStart=/usr/local/bin/mihomo "));
    }

    #[test]
    fn user_service_does_not_request_system_capabilities() {
        let unit = build_unit_content(
            Path::new("/home/me/.config/clash-cli/core/mihomo"),
            Path::new("/home/me/.config/clash-cli/runtime/config.yaml"),
            Path::new("/home/me/.config/clash-cli/runtime"),
            true,
            "clash-mihomo.service",
        );
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("AmbientCapabilities="));
        assert!(!unit.contains("CapabilityBoundingSet="));
    }

    #[test]
    fn system_service_can_read_user_owned_runtime_config() {
        let unit = build_unit_content(
            Path::new("/usr/local/bin/mihomo"),
            Path::new("/etc/clash-cli/runtime/config.yaml"),
            Path::new("/etc/clash-cli/runtime"),
            false,
            "clash-mihomo.service",
        );
        assert!(unit.contains("AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_DAC_OVERRIDE"));
        assert!(unit.contains("CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_DAC_OVERRIDE"));
    }

    #[test]
    fn launchd_plist_uses_core_current_link_not_usr_local_copy() {
        let core_current = PathBuf::from("/Users/me/.config/clash-cli/core/mihomo");
        let binary = resolve_service_binary(None, &core_current);
        let plist = build_launchd_plist(
            &binary,
            Path::new("/Users/me/.config/clash-cli/runtime/config.yaml"),
            Path::new("/Users/me/.config/clash-cli/runtime"),
            "com.clash-cli.clash-mihomo",
        );
        assert!(plist.contains("<string>/Users/me/.config/clash-cli/core/mihomo</string>"));
        assert!(plist.contains("<string>-d</string>"));
        assert!(plist.contains("<string>/Users/me/.config/clash-cli/runtime</string>"));
        assert!(plist.contains("<string>-f</string>"));
        assert!(plist.contains("<string>/Users/me/.config/clash-cli/runtime/config.yaml</string>"));
        assert!(!plist.contains("/usr/local/bin/mihomo"));
        assert_eq!(launchd_label("clash-mihomo"), "com.clash-cli.clash-mihomo");
        assert_eq!(
            launchd_label("clash-mihomo.service"),
            "com.clash-cli.clash-mihomo"
        );
        let parsed = launchd_plist_config_path(&plist).expect("plist 应能解析 -f");
        assert_eq!(
            parsed,
            PathBuf::from("/Users/me/.config/clash-cli/runtime/config.yaml")
        );
    }
}
