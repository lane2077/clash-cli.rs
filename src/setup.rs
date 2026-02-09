use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::api;
use crate::cli::{
    ApiCommand, ApiCommonArgs, CoreCommand, CoreInstallArgs, ProfileAddArgs, ProfileCommand,
    ProfileFetchArgs, ProfileRenderArgs, ProfileUseArgs, ServiceCommand, ServiceInstallArgs,
    ServiceTargetArgs, SetupCommand, SetupInitArgs, TunApplyArgs, TunCommand,
};
use crate::core;
use crate::output::is_json_mode;
use crate::paths::app_paths;
use crate::profile;
use crate::service;
use crate::tun;

const DEFAULT_SYSTEM_HOME: &str = "/etc/clash-cli";

pub fn run(command: SetupCommand) -> Result<()> {
    match command {
        SetupCommand::Init(args) => cmd_init(args),
    }
}

fn cmd_init(args: SetupInitArgs) -> Result<()> {
    ensure_linux_host()?;
    ensure_root_user()?;
    if is_json_mode() {
        bail!("`setup init` 暂不支持 --json，请先去掉 --json 执行");
    }
    if args.profile_url.trim().is_empty() {
        bail!("`--profile-url` 不能为空");
    }

    ensure_setup_home_for_root();

    println!("开始执行一键初始化...");

    core::run(CoreCommand::Install(CoreInstallArgs {
        version: args.core_version.clone(),
        mirror: args.mirror,
        amd64_variant: args.amd64_variant,
        force: args.force_core,
    }))?;

    let paths = app_paths()?;
    if !paths.core_current_link.exists() {
        bail!(
            "内核安装后未找到当前软链: {}",
            paths.core_current_link.display()
        );
    }

    install_binary(&paths.core_current_link, &args.binary)?;
    println!("已安装 mihomo 到: {}", args.binary.display());

    ensure_profile_ready(&args.profile_name, &args.profile_url)?;
    profile::run(ProfileCommand::Render(ProfileRenderArgs {
        name: Some(args.profile_name.clone()),
        output: None,
        no_mixin: false,
        follow_subscription_port: false,
    }))?;

    service::run(ServiceCommand::Install(ServiceInstallArgs {
        target: ServiceTargetArgs {
            name: args.service_name.clone(),
            user: false,
        },
        binary: Some(args.binary.clone()),
        config: Some(paths.runtime_config_file.clone()),
        workdir: Some(args.workdir.clone()),
        force: true,
        no_enable: false,
        no_start: false,
    }))?;

    if args.no_tun {
        println!("已跳过 tun 开启（--no-tun）。");
    } else {
        tun::run(TunCommand::On(TunApplyArgs {
            name: args.service_name.clone(),
            user: false,
            no_restart: false,
        }))?;
    }

    println!();
    println!("初始化完成。");
    println!("配置目录: {}", paths.config_dir.display());
    println!("运行配置: {}", paths.runtime_config_file.display());
    println!(
        "服务名称: {}.service",
        trim_service_suffix(&args.service_name)
    );
    println!("工作目录: {}", args.workdir.display());
    api::run(ApiCommand::UiUrl(ApiCommonArgs {
        controller: None,
        secret: None,
        timeout_secs: 15,
    }))?;
    println!("可直接执行: clash proxy start");
    println!("当前终端生效: eval \"$(clash proxy env on)\"");
    Ok(())
}

fn ensure_profile_ready(name: &str, url: &str) -> Result<()> {
    let add_result = profile::run(ProfileCommand::Add(ProfileAddArgs {
        name: name.to_string(),
        url: url.to_string(),
        use_profile: true,
        no_fetch: false,
    }));
    match add_result {
        Ok(()) => Ok(()),
        Err(err) => {
            if err.to_string().contains("profile 已存在") {
                println!("profile 已存在，执行强制拉取并切换: {}", name);
                profile::run(ProfileCommand::Fetch(ProfileFetchArgs {
                    name: name.to_string(),
                    force: true,
                }))?;
                profile::run(ProfileCommand::Use(ProfileUseArgs {
                    name: name.to_string(),
                    apply: false,
                    fetch: false,
                    service_name: "clash-mihomo".to_string(),
                    no_restart: true,
                }))?;
                return Ok(());
            }
            Err(err)
        }
    }
}

fn install_binary(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .with_context(|| format!("无效安装路径: {}", target.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("创建目录失败: {}", parent.display()))?;

    let tmp = target.with_extension("new");
    fs::copy(source, &tmp)
        .with_context(|| format!("复制内核失败: {} -> {}", source.display(), tmp.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("设置权限失败: {}", tmp.display()))?;
    fs::rename(&tmp, target)
        .with_context(|| format!("替换内核失败: {} -> {}", tmp.display(), target.display()))?;

    // SELinux 环境下尽量恢复上下文，失败不阻断。
    if command_exists("restorecon") {
        let _ = Command::new("restorecon").arg("-v").arg(target).status();
    }
    Ok(())
}

fn command_exists(binary: &str) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn trim_service_suffix(name: &str) -> String {
    name.strip_suffix(".service").unwrap_or(name).to_string()
}

fn ensure_linux_host() -> Result<()> {
    if env::consts::OS != "linux" {
        bail!("当前仅支持 Linux 平台");
    }
    Ok(())
}

fn ensure_root_user() -> Result<()> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("检测当前用户失败")?;
    if !output.status.success() {
        bail!("检测当前用户失败: id -u 返回非成功状态");
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid != "0" {
        bail!(
            "请使用 root 执行，例如: sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init --profile-url <URL>"
        );
    }
    Ok(())
}

fn ensure_setup_home_for_root() {
    if env::var_os("CLASH_CLI_HOME").is_none() {
        // 仅在命令主流程早期设置一次，后续同进程不会并发修改环境变量。
        unsafe {
            env::set_var("CLASH_CLI_HOME", DEFAULT_SYSTEM_HOME);
        }
        println!(
            "未设置 CLASH_CLI_HOME，setup init 默认使用系统目录: {}",
            DEFAULT_SYSTEM_HOME
        );
    }
}
