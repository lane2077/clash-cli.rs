use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::api;
use crate::cli::{
    Amd64Variant, ApiCommand, ApiCommonArgs, CoreCommand, CoreInstallArgs, MirrorSource,
    ProfileAddArgs, ProfileCommand, ProfileFetchArgs, ProfileRenderArgs, ProfileUseArgs,
    ServiceCommand, ServiceInstallArgs, ServiceTargetArgs, SetupCommand, SetupInitArgs,
    SetupUnifyArgs, TunApplyArgs, TunCommand,
};
use crate::core;
use crate::output::is_json_mode;
use crate::paths::app_paths;
use crate::profile;
use crate::service;
use crate::tun;

const DEFAULT_SYSTEM_HOME: &str = "/etc/clash-cli";
const AUTO_SUDO_ENV: &str = "CLASH_CLI_SUDO_REEXEC";

pub fn run(command: SetupCommand) -> Result<()> {
    match command {
        SetupCommand::Init(args) => cmd_init(args),
        SetupCommand::Unify(args) => cmd_unify(args),
    }
}

fn cmd_init(args: SetupInitArgs) -> Result<()> {
    ensure_linux_host()?;
    if ensure_setup_privileges_or_delegate(SetupAction::Init(&args))? == PrivilegeCheck::Delegated {
        return Ok(());
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileEntry {
    name: String,
    url: String,
    file: String,
    created_at: u64,
    updated_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProfileIndex {
    active: Option<String>,
    profiles: Vec<ProfileEntry>,
}

#[derive(Default)]
struct UnifyStats {
    imported: usize,
    existed: usize,
    conflicts: usize,
    missing_files: usize,
}

#[derive(Default)]
struct LinkStats {
    linked: usize,
    already_linked: usize,
    failed: usize,
}

fn cmd_unify(args: SetupUnifyArgs) -> Result<()> {
    ensure_linux_host()?;
    if ensure_setup_privileges_or_delegate(SetupAction::Unify(&args))? == PrivilegeCheck::Delegated
    {
        return Ok(());
    }
    ensure_root_user()?;
    if is_json_mode() {
        bail!("`setup unify` 暂不支持 --json，请先去掉 --json 执行");
    }

    ensure_setup_home_for_root();
    let paths = app_paths()?;
    fs::create_dir_all(&paths.profile_dir).context("创建目标 profile 目录失败")?;

    let mut index = load_profile_index(&paths.profile_index_file)?;
    let mut stats = UnifyStats::default();
    let mut warnings = Vec::<String>::new();
    let source_dirs = discover_source_config_dirs(&paths.config_dir)?;
    let mut candidate_active = index.active.clone();

    println!("开始收敛配置到: {}", paths.config_dir.display());
    if source_dirs.is_empty() {
        println!("未发现可合并的历史配置目录。");
    }

    for src_dir in &source_dirs {
        let src_profile_dir = src_dir.join("profiles");
        let src_index_file = src_profile_dir.join("index.json");
        if !src_index_file.exists() {
            continue;
        }
        let src_index = load_profile_index(&src_index_file).with_context(|| {
            format!(
                "读取来源 profile 索引失败: {}",
                src_index_file.display()
            )
        })?;
        println!("发现来源目录: {}", src_dir.display());
        if candidate_active.is_none() {
            candidate_active = src_index.active.clone();
        }

        for entry in src_index.profiles {
            merge_profile_entry(
                &entry,
                &src_profile_dir,
                &paths.profile_dir,
                &mut index,
                &mut stats,
                &mut warnings,
            )?;
        }
    }

    if index.active.is_none() {
        if let Some(active) = candidate_active {
            if index.profiles.iter().any(|p| p.name == active) {
                index.active = Some(active);
            }
        }
    }
    if index.active.is_none() && !index.profiles.is_empty() {
        index.active = Some(index.profiles[0].name.clone());
    }

    save_profile_index(&paths.profile_index_file, &index)?;
    println!(
        "收敛完成: imported={}, existed={}, conflicts={}, missing_files={}",
        stats.imported, stats.existed, stats.conflicts, stats.missing_files
    );
    if args.no_link {
        println!("已按请求跳过目录软链接替换（--no-link）。");
    } else {
        let link_stats = link_source_dirs_to_system(&source_dirs, &paths.config_dir, &mut warnings);
        println!(
            "目录收敛: linked={}, already_linked={}, failed={}",
            link_stats.linked, link_stats.already_linked, link_stats.failed
        );
    }
    if let Some(active) = index.active.as_deref() {
        println!("当前 active profile: {}", active);
    }

    for w in warnings {
        eprintln!("警告: {w}");
    }

    if args.no_apply {
        println!("已按请求跳过渲染与服务重启（--no-apply）。");
        return Ok(());
    }

    let active = index
        .active
        .clone()
        .context("收敛后没有可用 active profile，无法 apply")?;
    profile::run(ProfileCommand::Render(ProfileRenderArgs {
        name: Some(active),
        output: None,
        no_mixin: false,
        follow_subscription_port: false,
    }))?;
    service::run(ServiceCommand::Restart(ServiceTargetArgs {
        name: args.service_name.clone(),
        user: false,
    }))?;
    println!(
        "已渲染并重启服务: {}.service",
        trim_service_suffix(&args.service_name)
    );
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
    if !is_root_user()? {
        bail!(
            "请使用 root 执行，例如: sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init --profile-url <URL>"
        );
    }
    Ok(())
}

fn is_root_user() -> Result<bool> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("检测当前用户失败")?;
    if !output.status.success() {
        bail!("检测当前用户失败: id -u 返回非成功状态");
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(uid == "0")
}

fn ensure_setup_home_for_root() {
    if env::var_os("CLASH_CLI_HOME").is_none() {
        // 仅在命令主流程早期设置一次，后续同进程不会并发修改环境变量。
        unsafe {
            env::set_var("CLASH_CLI_HOME", DEFAULT_SYSTEM_HOME);
        }
        println!(
            "未设置 CLASH_CLI_HOME，默认使用系统目录: {}",
            DEFAULT_SYSTEM_HOME
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrivilegeCheck {
    Ok,
    Delegated,
}

enum SetupAction<'a> {
    Init(&'a SetupInitArgs),
    Unify(&'a SetupUnifyArgs),
}

fn ensure_setup_privileges_or_delegate(action: SetupAction<'_>) -> Result<PrivilegeCheck> {
    if is_root_user().unwrap_or(false) {
        return Ok(PrivilegeCheck::Ok);
    }
    if !should_auto_delegate_to_sudo() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !is_json_mode() {
        let action_name = match action {
            SetupAction::Init(_) => "setup init",
            SetupAction::Unify(_) => "setup unify",
        };
        println!("检测到权限不足，正在请求 sudo 授权继续执行 `clash {action_name}` ...");
    }

    let status = run_setup_with_sudo(action).context("调用 sudo 执行 setup 命令失败")?;
    if status.success() {
        return Ok(PrivilegeCheck::Delegated);
    }
    bail!("sudo 授权未通过或命令执行失败，请手动使用 sudo 重试");
}

fn should_auto_delegate_to_sudo() -> bool {
    if is_json_mode() {
        return false;
    }
    if env::var_os("CLASH_CLI_NO_AUTO_SUDO").is_some() {
        return false;
    }
    if env::var(AUTO_SUDO_ENV).ok().as_deref() == Some("1") {
        return false;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    command_exists("sudo")
}

fn run_setup_with_sudo(action: SetupAction<'_>) -> Result<std::process::ExitStatus> {
    let exe = std::env::current_exe().context("获取当前可执行文件路径失败")?;
    let mut cmd = Command::new("sudo");
    cmd.arg("env");
    cmd.arg(format!("{AUTO_SUDO_ENV}=1"));
    if let Some(home) = env::var_os("CLASH_CLI_HOME") {
        cmd.arg(format!("CLASH_CLI_HOME={}", home.to_string_lossy()));
    }
    cmd.arg(exe);
    if is_json_mode() {
        cmd.arg("--json");
    }
    cmd.arg("setup");
    match action {
        SetupAction::Init(args) => append_setup_init_args(&mut cmd, args),
        SetupAction::Unify(args) => append_setup_unify_args(&mut cmd, args),
    }
    cmd.status().context("启动 sudo 失败")
}

fn append_setup_init_args(cmd: &mut Command, args: &SetupInitArgs) {
    cmd.arg("init");
    cmd.arg("--profile-url").arg(&args.profile_url);
    cmd.arg("--profile-name").arg(&args.profile_name);
    cmd.arg("--core-version").arg(&args.core_version);
    cmd.arg("--mirror").arg(mirror_source_str(args.mirror));
    cmd.arg("--amd64-variant")
        .arg(amd64_variant_str(args.amd64_variant));
    if args.force_core {
        cmd.arg("--force-core");
    }
    cmd.arg("--binary").arg(&args.binary);
    cmd.arg("--workdir").arg(&args.workdir);
    cmd.arg("--service-name").arg(&args.service_name);
    if args.no_tun {
        cmd.arg("--no-tun");
    }
}

fn append_setup_unify_args(cmd: &mut Command, args: &SetupUnifyArgs) {
    cmd.arg("unify");
    cmd.arg("--service-name").arg(&args.service_name);
    if args.no_apply {
        cmd.arg("--no-apply");
    }
    if args.no_link {
        cmd.arg("--no-link");
    }
}

fn mirror_source_str(v: MirrorSource) -> &'static str {
    match v {
        MirrorSource::Auto => "auto",
        MirrorSource::Github => "github",
        MirrorSource::Ghfast => "ghfast",
    }
}

fn amd64_variant_str(v: Amd64Variant) -> &'static str {
    match v {
        Amd64Variant::Auto => "auto",
        Amd64Variant::Compatible => "compatible",
        Amd64Variant::V3 => "v3",
    }
}

fn discover_source_config_dirs(dest_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::<PathBuf>::new();

    if let Ok(user) = env::var("SUDO_USER") {
        if !user.is_empty() && user != "root" {
            if let Some(home) = lookup_home_by_user(&user)? {
                result.push(home.join(".config").join("clash-cli"));
            } else {
                result.push(PathBuf::from(format!("/home/{user}/.config/clash-cli")));
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        result.push(home.join(".config").join("clash-cli"));
    }
    result.push(PathBuf::from("/root/.config/clash-cli"));

    let mut unique = Vec::<PathBuf>::new();
    for dir in result {
        if dir == dest_dir {
            continue;
        }
        if !dir.exists() {
            continue;
        }
        if unique.iter().any(|x| x == &dir) {
            continue;
        }
        unique.push(dir);
    }
    Ok(unique)
}

fn lookup_home_by_user(user: &str) -> Result<Option<PathBuf>> {
    let output = Command::new("getent")
        .arg("passwd")
        .arg(user)
        .output()
        .context("执行 getent passwd 失败")?;
    if !output.status.success() {
        return Ok(None);
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let first = line.lines().next().unwrap_or_default();
    let fields = first.split(':').collect::<Vec<_>>();
    if fields.len() < 6 {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(fields[5])))
}

fn load_profile_index(path: &Path) -> Result<ProfileIndex> {
    if !path.exists() {
        return Ok(ProfileIndex::default());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("读取 profile 索引失败: {}", path.display()))?;
    serde_json::from_str(&content).context("解析 profile 索引失败")
}

fn save_profile_index(path: &Path, index: &ProfileIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(index).context("序列化 profile 索引失败")?;
    fs::write(path, content).with_context(|| format!("写入 profile 索引失败: {}", path.display()))
}

fn merge_profile_entry(
    entry: &ProfileEntry,
    src_profile_dir: &Path,
    dest_profile_dir: &Path,
    index: &mut ProfileIndex,
    stats: &mut UnifyStats,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let src_file = src_profile_dir.join(&entry.file);
    if !src_file.exists() {
        stats.missing_files += 1;
        warnings.push(format!(
            "来源 profile 缺少文件，已跳过: {} ({})",
            entry.name,
            src_file.display()
        ));
        return Ok(());
    }

    if let Some(existing) = index.profiles.iter_mut().find(|p| p.name == entry.name) {
        if existing.url != entry.url {
            stats.conflicts += 1;
            warnings.push(format!(
                "profile 名称冲突且 URL 不同，保留目标配置: {}",
                entry.name
            ));
            return Ok(());
        }
        if existing.updated_at.unwrap_or(0) < entry.updated_at.unwrap_or(0) {
            existing.updated_at = entry.updated_at;
        }
        let dst_file = dest_profile_dir.join(&existing.file);
        if !dst_file.exists() {
            fs::copy(&src_file, &dst_file).with_context(|| {
                format!("复制 profile 文件失败: {} -> {}", src_file.display(), dst_file.display())
            })?;
        }
        stats.existed += 1;
        return Ok(());
    }

    let mut imported = entry.clone();
    imported.file = format!("{}.yaml", imported.name);
    let mut dst_file = dest_profile_dir.join(&imported.file);
    if dst_file.exists() {
        imported.file = format!("{}-imported.yaml", imported.name);
        dst_file = dest_profile_dir.join(&imported.file);
    }
    fs::copy(&src_file, &dst_file).with_context(|| {
        format!(
            "复制 profile 文件失败: {} -> {}",
            src_file.display(),
            dst_file.display()
        )
    })?;
    index.profiles.push(imported);
    stats.imported += 1;
    Ok(())
}

fn link_source_dirs_to_system(
    source_dirs: &[PathBuf],
    dest_dir: &Path,
    warnings: &mut Vec<String>,
) -> LinkStats {
    let mut stats = LinkStats::default();
    for src_dir in source_dirs {
        let meta = match fs::symlink_metadata(src_dir) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if meta.file_type().is_symlink() {
            match fs::read_link(src_dir) {
                Ok(target) => {
                    if path_eq(&target, dest_dir) {
                        stats.already_linked += 1;
                        continue;
                    }
                }
                Err(err) => {
                    warnings.push(format!(
                        "读取软链接目标失败，准备重建: {} ({err})",
                        src_dir.display()
                    ));
                }
            }
        }

        let backup = build_backup_path(src_dir);
        if let Err(err) = fs::rename(src_dir, &backup) {
            warnings.push(format!(
                "目录替换失败（无法备份），已跳过: {} -> {} ({err})",
                src_dir.display(),
                backup.display()
            ));
            stats.failed += 1;
            continue;
        }

        if let Err(err) = symlink(dest_dir, src_dir) {
            let _ = fs::rename(&backup, src_dir);
            warnings.push(format!(
                "创建软链接失败，已回滚: {} -> {} ({err})",
                src_dir.display(),
                dest_dir.display()
            ));
            stats.failed += 1;
            continue;
        }
        stats.linked += 1;
    }
    stats
}

fn build_backup_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let base_name = path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("clash-cli"));
    let ts = now_unix();
    let mut idx: u32 = 0;
    loop {
        let mut name = OsString::from(base_name);
        if idx == 0 {
            name.push(format!(".bak.{ts}"));
        } else {
            name.push(format!(".bak.{ts}.{idx}"));
        }
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
        idx = idx.saturating_add(1);
    }
}

fn path_eq(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}
