use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::cli::{
    ProfileAddArgs, ProfileCommand, ProfileFetchArgs, ProfileRemoveArgs, ProfileRenderArgs,
    ProfileUseArgs, ProfileValidateArgs,
};
use crate::output::{is_json_mode, print_json};
use crate::paths::{AppPaths, app_paths};

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

const DEFAULT_LOCAL_MIXED_PORT: u16 = 7890;
const DEFAULT_LOCAL_SOCKS_PORT: u16 = 7891;
const DEFAULT_LOCAL_BIND_ADDRESS: &str = "127.0.0.1";
const DEFAULT_LOCAL_CONTROLLER: &str = "127.0.0.1:9090";
const DEFAULT_LOCAL_EXTERNAL_UI: &str = "ui";
const DEFAULT_LOCAL_EXTERNAL_UI_NAME: &str = "metacubexd";
const DEFAULT_LOCAL_EXTERNAL_UI_URL: &str =
    "https://ghfast.top/https://github.com/MetaCubeX/metacubexd/archive/refs/heads/gh-pages.zip";
const DEFAULT_SYSTEM_SERVICE_NAME: &str = "clash-mihomo.service";

pub fn run(command: ProfileCommand) -> Result<()> {
    match command {
        ProfileCommand::Add(args) => cmd_add(args),
        ProfileCommand::List => cmd_list(),
        ProfileCommand::Use(args) => cmd_use(args),
        ProfileCommand::Fetch(args) => cmd_fetch(args),
        ProfileCommand::Remove(args) => cmd_remove(args),
        ProfileCommand::Render(args) => cmd_render(args),
        ProfileCommand::Validate(args) => cmd_validate(args),
    }
}

fn cmd_add(args: ProfileAddArgs) -> Result<()> {
    validate_profile_name(&args.name)?;
    let paths = app_paths()?;
    let mut index = load_index(&paths.profile_index_file)?;

    if index.profiles.iter().any(|p| p.name == args.name) {
        bail!("profile 已存在: {}", args.name);
    }

    let mut entry = ProfileEntry {
        name: args.name.clone(),
        url: args.url,
        file: format!("{}.yaml", args.name),
        created_at: now_unix(),
        updated_at: None,
    };

    if !args.no_fetch {
        fetch_profile_entry(&mut entry, &paths.profile_dir, true)?;
    }
    if args.use_profile {
        index.active = Some(entry.name.clone());
    }
    index.profiles.push(entry.clone());
    save_index(&paths.profile_index_file, &index)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.add",
            "profile": entry,
            "active": index.active,
        }));
    }

    println!("已添加 profile: {}", args.name);
    if !args.no_fetch {
        println!("已拉取订阅内容。");
    }
    if args.use_profile {
        println!("已设为当前 profile。");
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;

    if is_json_mode() {
        return print_json(&index);
    }

    if index.profiles.is_empty() {
        print_profile_home_hint(&paths);
        println!("暂无 profile。可执行 `clash profile add --name xxx --url ...`");
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
    let apply = args.apply || args.fetch;

    if apply && !args.no_restart {
        ensure_service_runtime_home_matches_current(&args.service_name, &paths.runtime_config_file)?;
    }

    let mut index = load_index(&paths.profile_index_file)?;

    if !index.profiles.iter().any(|p| p.name == args.name) {
        bail!("profile 不存在: {}", args.name);
    }
    index.active = Some(args.name.clone());
    save_index(&paths.profile_index_file, &index)?;

    if args.fetch {
        cmd_fetch(ProfileFetchArgs {
            name: args.name.clone(),
            force: true,
        })?;
    }
    if apply {
        cmd_render(ProfileRenderArgs {
            name: Some(args.name.clone()),
            output: None,
            no_mixin: false,
            follow_subscription_port: false,
        })?;
        if !args.no_restart {
            restart_system_service(&args.service_name)?;
        }
    }

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.use",
            "active": index.active,
            "applied": apply,
            "fetched": args.fetch,
            "restarted": apply && !args.no_restart,
            "service": normalize_unit_name(&args.service_name),
        }));
    }

    println!("当前 profile 已切换为: {}", args.name);
    if apply {
        println!("已渲染到运行配置。");
        if args.no_restart {
            println!("已跳过服务重启（--no-restart）。");
        } else {
            println!("已重启服务: {}", normalize_unit_name(&args.service_name));
        }
    } else {
        println!(
            "提示: 仅切换了 active profile；如需立即生效请执行 `clash profile use --name {} --apply`",
            args.name
        );
    }
    Ok(())
}

fn cmd_fetch(args: ProfileFetchArgs) -> Result<()> {
    let paths = app_paths()?;
    let mut index = load_index(&paths.profile_index_file)?;
    let profile_snapshot = {
        let profile = index
            .profiles
            .iter_mut()
            .find(|p| p.name == args.name)
            .context("profile 不存在")?;

        let profile_path = paths.profile_dir.join(&profile.file);
        if !args.force && profile.updated_at.is_some() && profile_path.exists() {
            if now_unix().saturating_sub(profile.updated_at.unwrap_or(0)) < 60 {
                if is_json_mode() {
                    return print_json(&serde_json::json!({
                        "ok": true,
                        "action": "profile.fetch",
                        "name": args.name,
                        "skipped": true,
                        "reason": "recently updated",
                    }));
                }
                println!("最近 60 秒内已更新，跳过拉取。可加 --force 强制更新。");
                return Ok(());
            }
        }

        fetch_profile_entry(profile, &paths.profile_dir, args.force)?;
        profile.clone()
    };

    save_index(&paths.profile_index_file, &index)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.fetch",
            "profile": profile_snapshot,
        }));
    }

    println!("profile 拉取成功: {}", args.name);
    Ok(())
}

fn cmd_remove(args: ProfileRemoveArgs) -> Result<()> {
    let paths = app_paths()?;
    let mut index = load_index(&paths.profile_index_file)?;

    let pos = index
        .profiles
        .iter()
        .position(|p| p.name == args.name)
        .context("profile 不存在")?;
    let removed = index.profiles.remove(pos);
    if index.active.as_deref() == Some(removed.name.as_str()) {
        index.active = None;
    }
    save_index(&paths.profile_index_file, &index)?;

    let profile_path = paths.profile_dir.join(removed.file);
    if profile_path.exists() {
        fs::remove_file(&profile_path)
            .with_context(|| format!("删除 profile 文件失败: {}", profile_path.display()))?;
    }

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.remove",
            "removed": args.name,
            "active": index.active,
        }));
    }

    println!("已删除 profile: {}", args.name);
    Ok(())
}

fn cmd_render(args: ProfileRenderArgs) -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;
    let selected = select_profile(&index, args.name.as_deref())?;
    let source_path = paths.profile_dir.join(&selected.file);
    if !source_path.exists() {
        bail!(
            "profile 文件不存在: {}，请先执行 `clash profile fetch --name {}`",
            source_path.display(),
            selected.name
        );
    }

    let mut root = load_yaml(&source_path)?;
    if !args.follow_subscription_port {
        apply_local_listener_defaults(&mut root);
    }
    if !args.no_mixin && paths.profile_mixin_file.exists() {
        let mixin = load_yaml(&paths.profile_mixin_file)?;
        deep_merge(&mut root, &mixin);
    }

    let output = args.output.unwrap_or(paths.runtime_config_file);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let rendered = serde_yaml::to_string(&root).context("序列化渲染结果失败")?;
    fs::write(&output, rendered)
        .with_context(|| format!("写入渲染配置失败: {}", output.display()))?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.render",
            "profile": selected.name,
            "output": output.display().to_string(),
            "follow_subscription_port": args.follow_subscription_port,
        }));
    }

    println!(
        "渲染完成: profile={} -> {}",
        selected.name,
        output.display()
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

fn cmd_validate(args: ProfileValidateArgs) -> Result<()> {
    let paths = app_paths()?;
    let index = load_index(&paths.profile_index_file)?;
    let selected = select_profile(&index, args.name.as_deref())?;
    let source_path = paths.profile_dir.join(&selected.file);
    if !source_path.exists() {
        bail!(
            "profile 文件不存在: {}，请先执行 `clash profile fetch --name {}`",
            source_path.display(),
            selected.name
        );
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

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": warnings.is_empty(),
            "action": "profile.validate",
            "profile": selected.name,
            "warnings": warnings,
        }));
    }

    if warnings.is_empty() {
        println!("profile 校验通过: {}", selected.name);
    } else {
        println!("profile 校验完成: {}", selected.name);
        for item in warnings {
            println!("- {}", item);
        }
    }
    Ok(())
}

fn validate_profile_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("profile 名称不能为空");
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
            bail!("profile 名称仅支持字母/数字/.-_");
        }
    }
    Ok(())
}

fn load_index(path: &Path) -> Result<ProfileIndex> {
    if !path.exists() {
        return Ok(ProfileIndex::default());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 profile 索引失败: {}", path.display()))?;
    serde_json::from_str(&content).context("解析 profile 索引失败")
}

fn save_index(path: &Path, index: &ProfileIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(index).context("序列化 profile 索引失败")?;
    fs::write(path, content).with_context(|| format!("写入 profile 索引失败: {}", path.display()))
}

fn fetch_profile_entry(entry: &mut ProfileEntry, profile_dir: &Path, _force: bool) -> Result<()> {
    fs::create_dir_all(profile_dir)
        .with_context(|| format!("创建目录失败: {}", profile_dir.display()))?;
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

    let body = response.text().context("读取订阅响应失败")?;
    let _: Value = serde_yaml::from_str(&body).context("订阅内容不是有效 YAML")?;

    let path = profile_dir.join(&entry.file);
    fs::write(&path, body).with_context(|| format!("写入 profile 文件失败: {}", path.display()))?;
    entry.updated_at = Some(now_unix());
    Ok(())
}

fn select_profile<'a>(index: &'a ProfileIndex, name: Option<&str>) -> Result<&'a ProfileEntry> {
    let target = if let Some(v) = name {
        v.to_string()
    } else {
        index
            .active
            .clone()
            .context("未指定 profile 且当前没有 active profile")?
    };
    index
        .profiles
        .iter()
        .find(|p| p.name == target)
        .with_context(|| format!("profile 不存在: {}", target))
}

fn load_yaml(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取 YAML 失败: {}", path.display()))?;
    serde_yaml::from_str(&content).with_context(|| format!("解析 YAML 失败: {}", path.display()))
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
    set_root_u16(root, "mixed-port", DEFAULT_LOCAL_MIXED_PORT);
    set_root_u16(root, "socks-port", DEFAULT_LOCAL_SOCKS_PORT);
    set_root_bool(root, "allow-lan", false);
    set_root_string(root, "bind-address", DEFAULT_LOCAL_BIND_ADDRESS);
    set_root_string(root, "external-controller", DEFAULT_LOCAL_CONTROLLER);
    set_root_string(root, "external-ui", DEFAULT_LOCAL_EXTERNAL_UI);
    set_root_string(root, "external-ui-name", DEFAULT_LOCAL_EXTERNAL_UI_NAME);
    set_root_string(root, "external-ui-url", DEFAULT_LOCAL_EXTERNAL_UI_URL);
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

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}

fn normalize_unit_name(name: &str) -> String {
    if name.ends_with(".service") {
        name.to_string()
    } else {
        format!("{name}.service")
    }
}

fn restart_system_service(name: &str) -> Result<()> {
    let unit = normalize_unit_name(name);
    let output = Command::new("systemctl")
        .arg("restart")
        .arg(&unit)
        .output()
        .with_context(|| format!("执行 systemctl restart 失败: {unit}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "已渲染配置，但重启 {} 失败: {} (stdout={}, stderr={})",
            unit,
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(())
}

fn ensure_service_runtime_home_matches_current(
    service_name: &str,
    current_runtime_config: &Path,
) -> Result<()> {
    let unit = normalize_unit_name(service_name);
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
            "\n请改用同一目录执行，例如:\n  sudo env CLASH_CLI_HOME={} clash profile use --name <profile> --fetch --apply --service-name {}",
            home.display(),
            trim_service_suffix(service_name)
        ));
    } else if let Some(home) = current_home {
        message.push_str(&format!(
            "\n当前目录来源: {}。请保证 service -f 与该目录一致，或改用 service 对应目录执行。",
            home.display()
        ));
    } else {
        message.push_str("\n请保证 service 的 -f 路径与 CLASH_CLI_HOME/runtime/config.yaml 指向同一份配置。");
    }

    bail!("配置目录不一致，已阻止继续执行。\n{message}");
}

fn detect_service_runtime_config_path(unit: &str) -> Result<Option<PathBuf>> {
    let output = Command::new("systemctl")
        .arg("show")
        .arg("-p")
        .arg("ExecStart")
        .arg("--value")
        .arg(unit)
        .output()
        .with_context(|| format!("读取 service ExecStart 失败: {unit}"))?;

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
    if is_json_mode() {
        return;
    }
    println!("当前配置目录: {}", paths.config_dir.display());
    if let Ok(Some(service_runtime_config)) =
        detect_service_runtime_config_path(DEFAULT_SYSTEM_SERVICE_NAME)
    {
        if !path_eq(&service_runtime_config, &paths.runtime_config_file) {
            println!(
                "提示: {} 当前使用配置: {}",
                DEFAULT_SYSTEM_SERVICE_NAME,
                service_runtime_config.display()
            );
            if let Some(home) = infer_home_from_runtime_config(&service_runtime_config) {
                println!(
                    "如需管理该服务，请使用: sudo env CLASH_CLI_HOME={} clash profile list",
                    home.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
}
