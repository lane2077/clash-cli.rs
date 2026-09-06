use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::auto_sudo;
use crate::cli::{
    ProfileAddArgs, ProfileCommand, ProfileFetchArgs, ProfileRemoveArgs, ProfileRenderArgs,
    ProfileUpdateArgs, ProfileUseArgs, ProfileValidateArgs,
};
use crate::constants;
use crate::machine::{ErrorCode, coded_error};
use crate::output::{is_machine_mode, print_machine};
use crate::paths::{AppPaths, app_paths};
use crate::utils;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProfileEntry {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) file: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ProfileIndex {
    pub(crate) active: Option<String>,
    pub(crate) profiles: Vec<ProfileEntry>,
}

pub fn run(command: ProfileCommand) -> Result<()> {
    // Mixin 子命令有自己的 auto_sudo 逻辑，直接转发
    if let ProfileCommand::Mixin {
        command: ref mixin_cmd,
    } = command
    {
        return crate::mixin::run(mixin_cmd.clone());
    }

    let retry_command = command.clone();
    let result = match command {
        ProfileCommand::Add(args) => cmd_add(args),
        ProfileCommand::List => cmd_list(),
        ProfileCommand::Use(args) => cmd_use(args),
        ProfileCommand::Fetch(args) => cmd_fetch(args),
        ProfileCommand::Update(args) => cmd_update(args),
        ProfileCommand::Remove(args) => cmd_remove(args),
        ProfileCommand::Render(args) => cmd_render(args),
        ProfileCommand::Validate(args) => cmd_validate(args),
        ProfileCommand::Mixin { .. } => unreachable!(),
    };

    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            if should_retry_with_sudo(&retry_command, &err) {
                if !is_machine_mode() {
                    println!("检测到权限不足，正在请求 sudo 授权继续执行 sub 命令...");
                }
                return run_profile_with_sudo(&retry_command);
            }
            Err(err)
        }
    }
}

fn cmd_add(args: ProfileAddArgs) -> Result<()> {
    validate_profile_name(&args.name)?;
    let paths = app_paths()?;
    let mut index = load_index(&paths.profile_index_file)?;

    if index.profiles.iter().any(|p| p.name == args.name) {
        return Err(coded_error(
            ErrorCode::ProfileAlreadyExists,
            format!("订阅已存在: {}", args.name),
        ));
    }

    let mut entry = ProfileEntry {
        name: args.name.clone(),
        url: args.url,
        file: format!("{}.yaml", args.name),
        created_at: utils::now_unix(),
        updated_at: None,
    };

    if args.fetch {
        fetch_profile_entry(&mut entry, &paths.profile_dir, true)?;
    }
    index.profiles.push(entry.clone());
    save_index(&paths.profile_index_file, &index)?;

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "subscription": entry,
            "active": index.active,
            "fetched": args.fetch,
        }));
    }

    println!("已添加订阅: {}", args.name);
    if args.fetch {
        println!("已拉取订阅内容。");
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "active": index.active,
            "subscriptions": index.profiles
        }));
    }

    if index.profiles.is_empty() {
        print_profile_home_hint(&paths);
        println!("暂无订阅。可执行 `clash sub add --name xxx --url ...`");
        println!("已有订阅后更新并生效: clash sub update");
        return Ok(());
    }

    print_profile_home_hint(&paths);
    for profile in index.profiles {
        let mark = if index.active.as_deref() == Some(profile.name.as_str()) {
            "*"
        } else {
            " "
        };
        println!(
            "{} {} -> {} ({})",
            mark,
            profile.name,
            profile.url,
            profile
                .updated_at
                .map(|v| format!("updated_at={v}"))
                .unwrap_or_else(|| "未拉取".to_string())
        );
    }
    Ok(())
}

fn cmd_use(args: ProfileUseArgs) -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;
    let selected = index
        .profiles
        .iter()
        .find(|profile| profile.name == args.name)
        .ok_or_else(|| {
            coded_error(
                ErrorCode::ProfileNotFound,
                format!("订阅不存在: {}", args.name),
            )
        })?;
    let selected_path = paths.profile_dir.join(&selected.file);
    if !selected_path.is_file() {
        return Err(coded_error(
            ErrorCode::ProfileNotReady,
            format!(
                "订阅 `{}` 尚未拉取；请先执行 `clash sub fetch --name {}`",
                args.name, args.name
            ),
        ));
    }

    let applied = apply_subscription(
        &paths,
        ApplySpec {
            name: Some(args.name.clone()),
            fetch: false,
            render: true,
            restart: !args.no_restart,
            service_name: args.service_name.clone(),
        },
    )?;

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "active": applied.name,
            "restarted": applied.restarted,
            "service": utils::normalize_unit_name(&args.service_name),
        }));
    }

    println!("当前运行订阅已切换为: {}", args.name);
    if args.no_restart {
        println!("已跳过服务重启（--no-restart）。");
    } else {
        println!(
            "已重启服务: {}",
            utils::normalize_unit_name(&args.service_name)
        );
    }
    Ok(())
}

fn cmd_update(args: ProfileUpdateArgs) -> Result<()> {
    let paths = app_paths()?;
    let applied = apply_subscription(
        &paths,
        ApplySpec {
            name: args.name.clone(),
            fetch: true,
            render: true,
            restart: !args.no_restart,
            service_name: args.service_name.clone(),
        },
    )?;
    let name = applied.name;
    let restarted = applied.restarted;

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "subscription": name,
            "fetched": true,
            "applied": true,
            "restarted": restarted,
            "service": utils::normalize_unit_name(&args.service_name),
        }));
    }

    println!("已更新并渲染: {}", name);
    if restarted {
        println!(
            "已重启服务: {}",
            utils::normalize_unit_name(&args.service_name)
        );
    } else if args.no_restart {
        println!("已跳过服务重启（--no-restart）。");
    }
    Ok(())
}

struct FetchOutcome {
    skipped: bool,
    profile: ProfileEntry,
}

fn cmd_fetch(args: ProfileFetchArgs) -> Result<()> {
    let outcome = fetch_profile_named(&args.name, args.force)?;
    if is_machine_mode() {
        if outcome.skipped {
            return print_machine(&serde_json::json!({
                "name": args.name,
                "skipped": true,
                "reason": "recently updated",
            }));
        }
        return print_machine(&serde_json::json!({
            "subscription": outcome.profile,
        }));
    }

    if outcome.skipped {
        println!("最近 60 秒内已更新，跳过拉取。可加 --force 强制更新。");
    } else {
        println!("订阅拉取成功: {}", args.name);
    }
    Ok(())
}

fn fetch_profile_named(name: &str, force: bool) -> Result<FetchOutcome> {
    fetch_profile_in(&app_paths()?, name, force)
}

fn fetch_profile_in(paths: &AppPaths, name: &str, force: bool) -> Result<FetchOutcome> {
    let mut index = load_index(&paths.profile_index_file)?;
    let outcome = {
        let profile = index
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                coded_error(ErrorCode::ProfileNotFound, format!("订阅不存在: {name}"))
            })?;

        let profile_path = paths.profile_dir.join(&profile.file);
        if !force
            && profile.updated_at.is_some()
            && profile_path.exists()
            && utils::now_unix().saturating_sub(profile.updated_at.unwrap_or(0)) < 60
        {
            return Ok(FetchOutcome {
                skipped: true,
                profile: profile.clone(),
            });
        }

        fetch_profile_entry(profile, &paths.profile_dir, force)?;
        FetchOutcome {
            skipped: false,
            profile: profile.clone(),
        }
    };

    save_index(&paths.profile_index_file, &index)?;
    Ok(outcome)
}

fn cmd_remove(args: ProfileRemoveArgs) -> Result<()> {
    let paths = app_paths()?;
    let mut index = load_index(&paths.profile_index_file)?;

    if index.active.as_deref() == Some(args.name.as_str()) {
        return Err(coded_error(
            ErrorCode::StateConflict,
            format!(
                "不能删除当前运行订阅 `{}`；请先 render 另一订阅，使 active/runtime 一起切换",
                args.name
            ),
        ));
    }

    let pos = index
        .profiles
        .iter()
        .position(|p| p.name == args.name)
        .ok_or_else(|| {
            coded_error(
                ErrorCode::ProfileNotFound,
                format!("订阅不存在: {}", args.name),
            )
        })?;
    let removed = index.profiles.remove(pos);
    save_index(&paths.profile_index_file, &index)?;

    let profile_path = paths.profile_dir.join(removed.file);
    let reserved_mixin_file = profile_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("mixin.yaml"))
        .unwrap_or(false);
    if profile_path.exists() && !reserved_mixin_file {
        fs::remove_file(&profile_path)
            .with_context(|| format!("删除 profile 文件失败: {}", profile_path.display()))?;
    }

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "removed": args.name,
            "active": index.active,
        }));
    }

    println!("已删除订阅: {}", args.name);
    Ok(())
}

struct RenderOutcome {
    profile: String,
    output: PathBuf,
}

fn cmd_render(args: ProfileRenderArgs) -> Result<()> {
    let commit_active = args.output.is_none();
    let outcome = render_profile_to_runtime(&args)?;
    if commit_active {
        let paths = app_paths()?;
        let mut index = load_index(&paths.profile_index_file)?;
        index.active = Some(outcome.profile.clone());
        save_index(&paths.profile_index_file, &index)?;
    }
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "subscription": outcome.profile,
            "output": outcome.output.display().to_string(),
            "follow_subscription_port": args.follow_subscription_port,
        }));
    }

    println!(
        "渲染完成: profile={} -> {}",
        outcome.profile,
        outcome.output.display()
    );
    if args.follow_subscription_port {
        println!("已保留订阅中的监听端口设置。");
    } else {
        println!(
            "已应用本地默认值（mixed=7890, socks=7891, controller=127.0.0.1:9090, ui=metacubexd）。"
        );
    }
    Ok(())
}

fn render_profile_to_runtime(args: &ProfileRenderArgs) -> Result<RenderOutcome> {
    render_profile_in(&app_paths()?, args)
}

fn render_profile_in(paths: &AppPaths, args: &ProfileRenderArgs) -> Result<RenderOutcome> {
    let index = load_index(&paths.profile_index_file)?;
    let selected = select_profile(&index, args.name.as_deref())?;
    let source_path = paths.profile_dir.join(&selected.file);
    if !source_path.exists() {
        return Err(coded_error(
            ErrorCode::ProfileNotReady,
            format!(
                "订阅 `{}` 尚未拉取；请先执行 `clash sub fetch --name {}`",
                selected.name, selected.name
            ),
        ));
    }

    let root = load_yaml(&source_path)?;
    validate_subscription_root(&root)?;
    let mixin = if !args.no_mixin && paths.profile_mixin_file.exists() {
        Some(load_yaml(&paths.profile_mixin_file)?)
    } else {
        None
    };
    let root = merge_subscription_overlay(root, mixin.as_ref(), args.follow_subscription_port);

    let output = args
        .output
        .clone()
        .unwrap_or_else(|| paths.runtime_config_file.clone());
    write_yaml_file(&output, &root)?;
    Ok(RenderOutcome {
        profile: selected.name.clone(),
        output,
    })
}

fn cmd_validate(args: ProfileValidateArgs) -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;
    let selected = select_profile(&index, args.name.as_deref())?;
    let source_path = paths.profile_dir.join(&selected.file);
    if !source_path.exists() {
        return Err(coded_error(
            ErrorCode::ProfileNotReady,
            format!(
                "订阅 `{}` 尚未拉取；请先执行 `clash sub fetch --name {}`",
                selected.name, selected.name
            ),
        ));
    }

    let root = load_yaml(&source_path)?;
    let has_proxies = key_exists(&root, "proxies") || key_exists(&root, "proxy-providers");
    let has_rules = key_exists(&root, "rules");

    let mut warnings = Vec::<String>::new();
    if !has_proxies {
        warnings.push("未检测到 proxies/proxy-providers".to_string());
    }
    if !has_rules {
        warnings.push("未检测到 rules".to_string());
    }

    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "subscription": selected.name,
            "warnings": warnings,
        }));
    }

    if warnings.is_empty() {
        println!("订阅校验通过: {}", selected.name);
    } else {
        println!("订阅校验完成: {}", selected.name);
        for item in warnings {
            println!("- {}", item);
        }
    }
    Ok(())
}

fn validate_profile_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(coded_error(ErrorCode::ProfileInvalid, "订阅名称不能为空"));
    }
    if name.eq_ignore_ascii_case("mixin") {
        return Err(coded_error(
            ErrorCode::ProfileInvalid,
            "订阅名称 `mixin` 为保留名称，请换一个名称",
        ));
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
            return Err(coded_error(
                ErrorCode::ProfileInvalid,
                "订阅名称仅支持字母/数字/.-_",
            ));
        }
    }
    Ok(())
}

pub(crate) fn load_index(path: &Path) -> Result<ProfileIndex> {
    if !path.exists() {
        return Ok(ProfileIndex::default());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取订阅索引失败: {}", path.display()))?;
    serde_json::from_str(&content).map_err(|err| {
        coded_error(
            ErrorCode::ConfigInvalid,
            format!("解析订阅索引失败 {}: {err}", path.display()),
        )
    })
}

pub(crate) fn save_index(path: &Path, index: &ProfileIndex) -> Result<()> {
    let content = serde_json::to_string_pretty(index).context("序列化 订阅索引失败")?;
    utils::write_atomic_text(path, &content)
        .with_context(|| format!("写入 订阅索引失败: {}", path.display()))
}

fn local_subscription_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    let path = Path::new(url);
    if path.is_file() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn fetch_profile_entry(entry: &mut ProfileEntry, profile_dir: &Path, _force: bool) -> Result<()> {
    if entry.file.eq_ignore_ascii_case("mixin.yaml") {
        return Err(coded_error(
            ErrorCode::ProfileInvalid,
            "检测到保留名称订阅 `mixin`；为避免覆盖本地 mixin，请先改名后再拉取",
        ));
    }
    fs::create_dir_all(profile_dir)
        .with_context(|| format!("创建目录失败: {}", profile_dir.display()))?;

    let body = if let Some(local) = local_subscription_path(&entry.url) {
        fs::read_to_string(&local)
            .with_context(|| format!("读取本地订阅失败: {}", local.display()))?
    } else {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .context("创建 HTTP 客户端失败")?;

        let response = client
            .get(entry.url.clone())
            .send()
            .with_context(|| format!("请求订阅失败: {}", entry.url))?
            .error_for_status()
            .with_context(|| format!("订阅响应失败: {}", entry.url))?;

        response.text().context("读取订阅响应失败")?
    };
    let root: Value = serde_yaml::from_str(&body).map_err(|err| {
        coded_error(
            ErrorCode::ProfileInvalid,
            format!("订阅内容不是有效 YAML: {err}"),
        )
    })?;
    validate_subscription_root(&root)?;

    let path = profile_dir.join(&entry.file);
    utils::write_atomic_text(&path, &body)
        .with_context(|| format!("写入 profile 文件失败: {}", path.display()))?;
    entry.updated_at = Some(utils::now_unix());
    Ok(())
}

fn validate_subscription_root(root: &Value) -> Result<()> {
    let Some(mapping) = root.as_mapping() else {
        return Err(coded_error(
            ErrorCode::ProfileInvalid,
            "订阅顶层必须是 YAML 对象，拒绝覆盖现有配置",
        ));
    };
    let has_proxy_source = ["proxies", "proxy-providers"]
        .iter()
        .any(|key| mapping.contains_key(Value::String((*key).to_string())));
    if !has_proxy_source {
        return Err(coded_error(
            ErrorCode::ProfileInvalid,
            "订阅缺少 proxies/proxy-providers，拒绝覆盖现有配置",
        ));
    }
    Ok(())
}

fn select_profile<'a>(index: &'a ProfileIndex, name: Option<&str>) -> Result<&'a ProfileEntry> {
    let target = if let Some(v) = name {
        v.to_string()
    } else {
        index
            .active
            .clone()
            .context("未指定订阅且当前没有 active 订阅")?
    };
    index
        .profiles
        .iter()
        .find(|p| p.name == target)
        .ok_or_else(|| coded_error(ErrorCode::ProfileNotFound, format!("订阅不存在: {target}")))
}

fn load_yaml(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取 YAML 失败: {}", path.display()))?;
    serde_yaml::from_str(&content).map_err(|err| {
        coded_error(
            ErrorCode::ConfigInvalid,
            format!("解析 YAML 失败 {}: {err}", path.display()),
        )
    })
}

fn load_runtime_or_empty(path: &Path) -> Result<Value> {
    if path.exists() {
        load_yaml(path)
    } else {
        Ok(Value::Mapping(serde_yaml::Mapping::new()))
    }
}

fn write_yaml_file(path: &Path, root: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let rendered = serde_yaml::to_string(root).context("序列化渲染结果失败")?;
    utils::write_atomic_text(path, &rendered)
        .with_context(|| format!("写入渲染配置失败: {}", path.display()))
}

/// 订阅生效：拉取、合成 runtime、重启服务。
/// `sub use` 与 `sub update` 穿过同一生效 seam，不在命令里各拼一遍流水线。
#[derive(Debug, Clone)]
pub struct ApplySpec {
    pub name: Option<String>,
    pub fetch: bool,
    pub render: bool,
    pub restart: bool,
    pub service_name: String,
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub name: String,
    pub fetched: bool,
    pub applied: bool,
    pub restarted: bool,
}

pub fn apply_subscription(paths: &AppPaths, spec: ApplySpec) -> Result<ApplyResult> {
    if spec.restart {
        ensure_service_runtime_home_matches_current(
            &spec.service_name,
            &paths.runtime_config_file,
        )?;
    }

    let index = load_index(&paths.profile_index_file)?;
    let selected = select_profile(&index, spec.name.as_deref())?;
    let name = selected.name.clone();

    let mut fetched = false;
    if spec.fetch {
        let outcome = fetch_profile_in(paths, &name, true)?;
        fetched = !outcome.skipped;
    }

    let mut applied = false;
    if spec.render {
        render_profile_in(
            paths,
            &ProfileRenderArgs {
                name: Some(name.clone()),
                output: None,
                no_mixin: false,
                follow_subscription_port: false,
            },
        )?;
        applied = true;
    }

    if spec.render {
        // “已经应用的 profile”必须与 runtime 来源一致。指定名称的 update 也会成为 active；
        // 仅想刷新但不应用时使用 `sub fetch`。
        let mut committed_index = load_index(&paths.profile_index_file)?;
        committed_index.active = Some(name.clone());
        save_index(&paths.profile_index_file, &committed_index)?;
    }

    let mut restarted = false;
    if spec.restart {
        restart_system_service(&spec.service_name)?;
        restarted = true;
    }

    Ok(ApplyResult {
        name,
        fetched,
        applied,
        restarted,
    })
}

/// 订阅 YAML + mixin overlay → 运行配置。`follow_subscription_port` 为 true 时不覆盖监听端口。
pub fn merge_subscription_overlay(
    mut root: Value,
    mixin: Option<&Value>,
    follow_subscription_port: bool,
) -> Value {
    if !follow_subscription_port {
        apply_local_listener_defaults(&mut root);
    }
    if let Some(mixin) = mixin {
        deep_merge(&mut root, mixin);
    }
    root
}

/// 将当前 active 订阅（若有）与 mixin 合成为 runtime/config.yaml。
pub fn render_runtime_from_home(paths: &AppPaths) -> Result<()> {
    let mixin = if paths.profile_mixin_file.exists() {
        Some(load_yaml(&paths.profile_mixin_file)?)
    } else {
        None
    };
    let index = load_index(&paths.profile_index_file)?;
    let (base, apply_listener_defaults) = match select_profile(&index, None) {
        Ok(selected) => {
            let source_path = paths.profile_dir.join(&selected.file);
            if source_path.exists() {
                let base = load_yaml(&source_path)?;
                validate_subscription_root(&base)?;
                (base, true)
            } else {
                (load_runtime_or_empty(&paths.runtime_config_file)?, false)
            }
        }
        Err(_) => (load_runtime_or_empty(&paths.runtime_config_file)?, false),
    };
    let rendered = merge_subscription_overlay(base, mixin.as_ref(), !apply_listener_defaults);
    write_yaml_file(&paths.runtime_config_file, &rendered)
}

fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Mapping(base_map), Value::Mapping(patch_map)) => {
            for (k, v) in patch_map {
                match base_map.get_mut(k) {
                    Some(base_val) => deep_merge(base_val, v),
                    None => {
                        base_map.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (base_val, patch_val) => {
            *base_val = patch_val.clone();
        }
    }
}

fn apply_local_listener_defaults(root: &mut Value) {
    set_root_u16(root, "mixed-port", constants::DEFAULT_MIXED_PORT);
    set_root_u16(root, "socks-port", constants::DEFAULT_SOCKS_PORT);
    set_root_bool(root, "allow-lan", false);
    set_root_string(root, "bind-address", constants::DEFAULT_BIND_ADDRESS);
    set_root_string(root, "external-controller", constants::DEFAULT_CONTROLLER);
    set_root_string(root, "external-ui", constants::DEFAULT_EXTERNAL_UI);
    set_root_string(
        root,
        "external-ui-name",
        constants::DEFAULT_EXTERNAL_UI_NAME,
    );
    set_root_string(root, "external-ui-url", constants::DEFAULT_EXTERNAL_UI_URL);
}

fn set_root_u16(root: &mut Value, key: &str, value: u16) {
    ensure_root_mapping(root).insert(
        Value::String(key.to_string()),
        Value::Number(serde_yaml::Number::from(value as i64)),
    );
}

fn set_root_bool(root: &mut Value, key: &str, value: bool) {
    ensure_root_mapping(root).insert(Value::String(key.to_string()), Value::Bool(value));
}

fn set_root_string(root: &mut Value, key: &str, value: &str) {
    ensure_root_mapping(root).insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
}

fn ensure_root_mapping(root: &mut Value) -> &mut serde_yaml::Mapping {
    if !root.is_mapping() {
        *root = Value::Mapping(serde_yaml::Mapping::new());
    }
    root.as_mapping_mut().expect("root mapping")
}

fn key_exists(root: &Value, key: &str) -> bool {
    root.as_mapping()
        .map(|m| m.contains_key(Value::String(key.to_string())))
        .unwrap_or(false)
}

fn restart_system_service(name: &str) -> Result<()> {
    crate::service::restart_managed_service(name, false)
        .with_context(|| format!("已渲染配置，但重启 {name} 失败"))
}

fn ensure_service_runtime_home_matches_current(
    service_name: &str,
    current_runtime_config: &Path,
) -> Result<()> {
    let unit = utils::normalize_unit_name(service_name);
    let Some(service_runtime_config) = detect_service_runtime_config_path(&unit)? else {
        return Ok(());
    };

    if path_eq(&service_runtime_config, current_runtime_config) {
        return Ok(());
    }

    let service_home = infer_home_from_runtime_config(&service_runtime_config);
    let current_home = infer_home_from_runtime_config(current_runtime_config);

    let mut message = format!(
        "检测到服务 {} 使用配置: {}\n当前命令使用配置: {}",
        unit,
        service_runtime_config.display(),
        current_runtime_config.display()
    );
    message.push_str("\n这会导致「profile 切换看似成功，但 Dashboard 仍显示旧配置」。");

    if let Some(home) = service_home {
        message.push_str(&format!(
            "\n请改用同一目录执行，例如:\n  sudo env CLASH_CLI_HOME={} clash sub update --name <name> --service-name {}",
            home.display(),
            trim_service_suffix(service_name)
        ));
    } else if let Some(home) = current_home {
        message.push_str(&format!(
            "\n当前目录来源: {}。请保证 service -f 与该目录一致，或改用 service 对应目录执行。",
            home.display()
        ));
    } else {
        message.push_str(
            "\n请保证 service 的 -f 路径与 CLASH_CLI_HOME/runtime/config.yaml 指向同一份配置。",
        );
    }

    Err(coded_error(
        ErrorCode::StateConflict,
        format!("配置目录不一致，已阻止继续执行。\n{message}"),
    ))
}

fn detect_service_runtime_config_path(unit: &str) -> Result<Option<PathBuf>> {
    if crate::utils::is_macos() {
        return Ok(detect_launchd_runtime_config_path(unit));
    }
    detect_systemd_runtime_config_path(unit)
}

fn detect_launchd_runtime_config_path(unit: &str) -> Option<PathBuf> {
    let label = crate::service::launchd_label(unit);
    for path in crate::service::launchd_plist_search_paths(&label) {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(cfg) = crate::service::launchd_plist_config_path(&content) {
            return Some(cfg);
        }
    }
    None
}

fn detect_systemd_runtime_config_path(unit: &str) -> Result<Option<PathBuf>> {
    if !utils::command_exists("systemctl") {
        return Ok(None);
    }
    let output = match Command::new("systemctl")
        .arg("show")
        .arg("-p")
        .arg("ExecStart")
        .arg("--value")
        .arg(unit)
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let exec = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if exec.is_empty() {
        return Ok(None);
    }

    let mut prev_is_f = false;
    for token in exec.split_whitespace() {
        let cleaned = token
            .trim_matches(|c| c == '"' || c == '\'')
            .trim_end_matches(';')
            .trim_end_matches(',');
        if cleaned.is_empty() {
            continue;
        }
        if prev_is_f {
            return Ok(Some(PathBuf::from(cleaned)));
        }
        prev_is_f = cleaned == "-f";
    }

    Ok(None)
}

fn path_eq(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn infer_home_from_runtime_config(path: &Path) -> Option<PathBuf> {
    if path.file_name()?.to_str()? != "config.yaml" {
        return None;
    }
    let runtime_dir = path.parent()?;
    if runtime_dir.file_name()?.to_str()? != "runtime" {
        return None;
    }
    runtime_dir.parent().map(|p| p.to_path_buf())
}

fn trim_service_suffix(name: &str) -> &str {
    name.strip_suffix(".service").unwrap_or(name)
}

fn print_profile_home_hint(paths: &AppPaths) {
    if is_machine_mode() {
        return;
    }
    println!("当前配置目录: {}", paths.config_dir.display());
    if let Ok(Some(service_runtime_config)) =
        detect_service_runtime_config_path(constants::DEFAULT_SYSTEM_SERVICE_UNIT)
        && !path_eq(&service_runtime_config, &paths.runtime_config_file)
    {
        println!(
            "提示: {} 当前使用配置: {}",
            constants::DEFAULT_SYSTEM_SERVICE_UNIT,
            service_runtime_config.display()
        );
        if let Some(home) = infer_home_from_runtime_config(&service_runtime_config) {
            println!(
                "如需管理该服务，请使用: sudo env CLASH_CLI_HOME={} clash sub list",
                home.display()
            );
        }
    }
}

fn should_retry_with_sudo(command: &ProfileCommand, err: &anyhow::Error) -> bool {
    if !profile_command_requires_write(command) {
        return false;
    }
    if !auto_sudo::is_permission_denied_error(err) {
        return false;
    }
    auto_sudo::should_auto_delegate(is_machine_mode())
}

fn profile_command_requires_write(command: &ProfileCommand) -> bool {
    matches!(
        command,
        ProfileCommand::Add(_)
            | ProfileCommand::Use(_)
            | ProfileCommand::Fetch(_)
            | ProfileCommand::Remove(_)
            | ProfileCommand::Render(_)
            | ProfileCommand::Update(_)
    )
}

fn run_profile_with_sudo(command: &ProfileCommand) -> Result<()> {
    let cli_args = profile_command_to_cli_args(command)?;
    let status = auto_sudo::run_with_sudo(is_machine_mode(), |cmd| {
        cmd.args(&cli_args);
        Ok(())
    })?;
    if status.success() {
        return Ok(());
    }
    bail!("sudo 授权未通过或命令执行失败，请手动使用 sudo 重试");
}

fn profile_command_to_cli_args(command: &ProfileCommand) -> Result<Vec<String>> {
    let mut args = vec!["sub".to_string()];
    match command {
        ProfileCommand::Add(v) => {
            args.push("add".to_string());
            args.push("--name".to_string());
            args.push(v.name.clone());
            args.push("--url".to_string());
            args.push(v.url.clone());
            if v.fetch {
                args.push("--fetch".to_string());
            }
        }
        ProfileCommand::List => {
            args.push("list".to_string());
        }
        ProfileCommand::Use(v) => {
            args.push("use".to_string());
            args.push("--name".to_string());
            args.push(v.name.clone());
            args.push("--service-name".to_string());
            args.push(v.service_name.clone());
            if v.no_restart {
                args.push("--no-restart".to_string());
            }
        }
        ProfileCommand::Fetch(v) => {
            args.push("fetch".to_string());
            args.push("--name".to_string());
            args.push(v.name.clone());
            if v.force {
                args.push("--force".to_string());
            }
        }
        ProfileCommand::Update(v) => {
            args.push("update".to_string());
            if let Some(name) = &v.name {
                args.push("--name".to_string());
                args.push(name.clone());
            }
            args.push("--service-name".to_string());
            args.push(v.service_name.clone());
            if v.no_restart {
                args.push("--no-restart".to_string());
            }
        }
        ProfileCommand::Remove(v) => {
            args.push("remove".to_string());
            args.push("--name".to_string());
            args.push(v.name.clone());
        }
        ProfileCommand::Render(v) => {
            args.push("render".to_string());
            if let Some(name) = &v.name {
                args.push("--name".to_string());
                args.push(name.clone());
            }
            if let Some(output) = &v.output {
                args.push("--output".to_string());
                args.push(output.display().to_string());
            }
            if v.no_mixin {
                args.push("--no-mixin".to_string());
            }
            if v.follow_subscription_port {
                args.push("--follow-subscription-port".to_string());
            }
        }
        ProfileCommand::Validate(v) => {
            args.push("validate".to_string());
            if let Some(name) = &v.name {
                args.push("--name".to_string());
                args.push(name.clone());
            }
        }
        ProfileCommand::Mixin { .. } => unreachable!(),
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_path(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_nanos())
            .unwrap_or(0);
        path.push(format!(
            "clash_cli_{prefix}_{}_{}",
            std::process::id(),
            nanos
        ));
        path
    }

    fn parse_yaml(input: &str) -> Value {
        serde_yaml::from_str(input).expect("解析测试 YAML 失败")
    }

    #[test]
    fn local_subscription_path_accepts_file_url_and_plain_path() {
        assert_eq!(
            local_subscription_path("file:///tmp/sub.yaml").as_deref(),
            Some(Path::new("/tmp/sub.yaml"))
        );
        assert_eq!(local_subscription_path("https://example.com/a.yaml"), None);
    }

    #[test]
    fn validate_profile_name_should_accept_valid_name() {
        assert!(validate_profile_name("default").is_ok());
        assert!(validate_profile_name("my-profile_1.2").is_ok());
    }

    #[test]
    fn validate_profile_name_should_reject_invalid_name() {
        assert!(validate_profile_name("").is_err());
        assert!(validate_profile_name("  ").is_err());
        assert!(validate_profile_name("abc def").is_err());
        assert!(validate_profile_name("ab/def").is_err());
        assert!(validate_profile_name("中文").is_err());
        assert!(validate_profile_name("mixin").is_err());
        assert!(validate_profile_name("MIXIN").is_err());
    }

    #[test]
    fn subscription_validation_rejects_scalar_and_error_mapping() {
        let scalar = parse_yaml("upstream temporarily unavailable\n");
        assert!(validate_subscription_root(&scalar).is_err());
        let error_mapping = parse_yaml("message: temporarily unavailable\n");
        assert!(validate_subscription_root(&error_mapping).is_err());
        let valid = parse_yaml("proxies: []\nrules: []\n");
        assert!(validate_subscription_root(&valid).is_ok());
    }

    #[test]
    fn deep_merge_should_merge_nested_mapping_and_replace_scalar() {
        let mut base = parse_yaml(
            r#"
mixed:
  keep: 1
  replace: old
arr:
  - 1
  - 2
scalar: old
"#,
        );
        let patch = parse_yaml(
            r#"
mixed:
  replace: new
  add: 2
arr:
  - 3
scalar:
  nested: true
"#,
        );
        let expected = parse_yaml(
            r#"
mixed:
  keep: 1
  replace: new
  add: 2
arr:
  - 3
scalar:
  nested: true
"#,
        );

        deep_merge(&mut base, &patch);
        assert_eq!(base, expected);
    }

    #[test]
    fn key_exists_should_detect_top_level_key() {
        let root = parse_yaml(
            r#"
proxies: []
mode: rule
"#,
        );
        assert!(key_exists(&root, "proxies"));
        assert!(!key_exists(&root, "rules"));
    }

    #[test]
    fn apply_local_listener_defaults_should_override_subscription_listener_keys() {
        let mut root = parse_yaml(
            r#"
allow-lan: true
mixed-port: 9981
socks-port: 9982
bind-address: 0.0.0.0
external-controller: 0.0.0.0:9091
"#,
        );

        apply_local_listener_defaults(&mut root);

        let map = root.as_mapping().expect("root 不是 mapping");
        assert_eq!(
            map.get(Value::String("allow-lan".to_string()))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            map.get(Value::String("mixed-port".to_string()))
                .and_then(|v| v.as_i64()),
            Some(7890)
        );
        assert_eq!(
            map.get(Value::String("socks-port".to_string()))
                .and_then(|v| v.as_i64()),
            Some(7891)
        );
        assert_eq!(
            map.get(Value::String("bind-address".to_string()))
                .and_then(|v| v.as_str()),
            Some("127.0.0.1")
        );
        assert_eq!(
            map.get(Value::String("external-controller".to_string()))
                .and_then(|v| v.as_str()),
            Some("127.0.0.1:9090")
        );
        assert_eq!(
            map.get(Value::String("external-ui".to_string()))
                .and_then(|v| v.as_str()),
            Some("ui")
        );
        assert_eq!(
            map.get(Value::String("external-ui-name".to_string()))
                .and_then(|v| v.as_str()),
            Some("metacubexd")
        );
        assert_eq!(
            map.get(Value::String("external-ui-url".to_string()))
                .and_then(|v| v.as_str()),
            Some(
                "https://ghfast.top/https://github.com/MetaCubeX/metacubexd/archive/refs/heads/gh-pages.zip"
            )
        );
    }

    #[test]
    fn save_and_load_index_should_round_trip() {
        let index_path = temp_path("profile_index").join("index.json");
        let index = ProfileIndex {
            active: Some("p1".to_string()),
            profiles: vec![ProfileEntry {
                name: "p1".to_string(),
                url: "https://example.com/sub.yaml".to_string(),
                file: "p1.yaml".to_string(),
                created_at: 1,
                updated_at: Some(2),
            }],
        };

        save_index(&index_path, &index).expect("保存索引失败");
        let loaded = load_index(&index_path).expect("读取索引失败");

        assert_eq!(loaded.active.as_deref(), Some("p1"));
        assert_eq!(loaded.profiles.len(), 1);
        let first = loaded.profiles.first().expect("profile 不存在");
        assert_eq!(first.name, "p1");
        assert_eq!(first.url, "https://example.com/sub.yaml");
        assert_eq!(first.file, "p1.yaml");
        assert_eq!(first.created_at, 1);
        assert_eq!(first.updated_at, Some(2));

        if let Some(parent) = index_path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn select_profile_should_use_active_when_name_missing() {
        let index = ProfileIndex {
            active: Some("active-p".to_string()),
            profiles: vec![
                ProfileEntry {
                    name: "active-p".to_string(),
                    url: "https://example.com/a.yaml".to_string(),
                    file: "active-p.yaml".to_string(),
                    created_at: 1,
                    updated_at: None,
                },
                ProfileEntry {
                    name: "other".to_string(),
                    url: "https://example.com/b.yaml".to_string(),
                    file: "other.yaml".to_string(),
                    created_at: 2,
                    updated_at: None,
                },
            ],
        };

        let selected = select_profile(&index, None).expect("按 active 选择失败");
        assert_eq!(selected.name, "active-p");

        let selected_by_name =
            select_profile(&index, Some("other")).expect("按名称选择 profile 失败");
        assert_eq!(selected_by_name.name, "other");
    }

    fn yaml_bool(root: &Value, path: &[&str]) -> Option<bool> {
        let mut current = root;
        for key in path {
            current = current
                .as_mapping()?
                .get(Value::String((*key).to_string()))?;
        }
        current.as_bool()
    }

    fn yaml_str(root: &Value, path: &[&str]) -> Option<String> {
        let mut current = root;
        for key in path {
            current = current
                .as_mapping()?
                .get(Value::String((*key).to_string()))?;
        }
        current.as_str().map(|s| s.to_string())
    }

    #[test]
    fn render_keeps_tun_on_overlay_when_subscription_omits_tun() {
        let subscription = parse_yaml(
            r#"
proxies: []
rules:
  - MATCH,DIRECT
"#,
        );
        let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
        crate::tun::apply_tun_policy_overlay_for(&mut overlay, true, "linux");
        let rendered = merge_subscription_overlay(subscription, Some(&overlay), false);
        assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
        assert_eq!(yaml_bool(&rendered, &["tun", "auto-redirect"]), Some(true));
        assert_eq!(yaml_bool(&rendered, &["tun", "auto-route"]), Some(true));
        assert_eq!(yaml_bool(&rendered, &["dns", "enable"]), Some(true));
        assert_eq!(
            yaml_str(&rendered, &["dns", "enhanced-mode"]).as_deref(),
            Some("fake-ip")
        );
    }

    #[test]
    fn render_keeps_tun_on_overlay_when_subscription_disables_tun() {
        let subscription = parse_yaml(
            r#"
tun:
  enable: false
proxies: []
rules:
  - MATCH,DIRECT
"#,
        );
        let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
        crate::tun::apply_tun_policy_overlay_for(&mut overlay, true, "linux");
        let rendered = merge_subscription_overlay(subscription, Some(&overlay), false);
        assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
        assert_eq!(yaml_bool(&rendered, &["tun", "auto-redirect"]), Some(true));
    }

    #[test]
    fn render_applies_tun_off_overlay() {
        let subscription = parse_yaml(
            r#"
tun:
  enable: true
  auto-redirect: true
proxies: []
"#,
        );
        let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
        crate::tun::apply_tun_policy_overlay(&mut overlay, true);
        crate::tun::apply_tun_policy_overlay(&mut overlay, false);
        let rendered = merge_subscription_overlay(subscription, Some(&overlay), false);
        assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(false));
    }
}
