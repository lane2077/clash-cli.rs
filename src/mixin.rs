use std::fs;

use anyhow::{Context, Result, bail};
use serde_yaml::Value;

use crate::auto_sudo;
use crate::cli::{MixinCommand, MixinSetArgs};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

pub fn run(command: MixinCommand) -> Result<()> {
    let retry_command = command.clone();
    let result = match command {
        MixinCommand::Show => cmd_show(),
        MixinCommand::Set(args) => cmd_set(args),
        MixinCommand::Unset(args) => cmd_unset(args),
        MixinCommand::Reset => cmd_reset(),
    };

    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            if mixin_command_requires_write(&retry_command)
                && auto_sudo::is_permission_denied_error(&err)
                && auto_sudo::should_auto_delegate(is_json_mode())
            {
                if !is_json_mode() {
                    println!("检测到权限不足，正在请求 sudo 授权继续执行 mixin 命令...");
                }
                return run_mixin_with_sudo(&retry_command);
            }
            Err(err)
        }
    }
}

fn cmd_show() -> Result<()> {
    let paths = app_paths()?;
    if !paths.profile_mixin_file.exists() {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "profile.mixin.show",
                "exists": false,
                "content": null,
            }));
        }
        println!(
            "mixin.yaml 不存在（路径: {}）",
            paths.profile_mixin_file.display()
        );
        println!("提示: 使用 `clash profile mixin set --key <key> --value <value>` 创建");
        return Ok(());
    }

    let content = fs::read_to_string(&paths.profile_mixin_file).with_context(|| {
        format!(
            "读取 mixin.yaml 失败: {}",
            paths.profile_mixin_file.display()
        )
    })?;

    if is_json_mode() {
        let parsed: Value = serde_yaml::from_str(&content).unwrap_or(Value::Null);
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.mixin.show",
            "exists": true,
            "content": yaml_to_json(&parsed),
        }));
    }

    println!("# mixin.yaml ({})", paths.profile_mixin_file.display());
    print!("{}", content);
    if !content.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn cmd_set(args: MixinSetArgs) -> Result<()> {
    let paths = app_paths()?;
    let mut root = load_mixin_or_empty(&paths.profile_mixin_file)?;

    let parsed_value = parse_yaml_value(&args.value);
    set_nested_key(&mut root, &args.key, parsed_value)?;

    save_mixin(&paths.profile_mixin_file, &root)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.mixin.set",
            "key": args.key,
            "value": args.value,
        }));
    }

    println!("已设置 mixin: {} = {}", args.key, args.value);
    println!("提示: 执行 `clash profile render` 使变更生效");
    Ok(())
}

fn cmd_unset(args: MixinSetArgs) -> Result<()> {
    let paths = app_paths()?;
    if !paths.profile_mixin_file.exists() {
        bail!("mixin.yaml 不存在，无需删除字段");
    }

    let mut root = load_mixin_or_empty(&paths.profile_mixin_file)?;
    let removed = unset_nested_key(&mut root, &args.key);

    if !removed {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "profile.mixin.unset",
                "key": args.key,
                "removed": false,
            }));
        }
        println!("字段不存在: {}", args.key);
        return Ok(());
    }

    save_mixin(&paths.profile_mixin_file, &root)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.mixin.unset",
            "key": args.key,
            "removed": true,
        }));
    }

    println!("已删除 mixin 字段: {}", args.key);
    println!("提示: 执行 `clash profile render` 使变更生效");
    Ok(())
}

fn cmd_reset() -> Result<()> {
    let paths = app_paths()?;
    if !paths.profile_mixin_file.exists() {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "profile.mixin.reset",
                "existed": false,
            }));
        }
        println!("mixin.yaml 不存在，无需重置。");
        return Ok(());
    }

    fs::remove_file(&paths.profile_mixin_file).with_context(|| {
        format!(
            "删除 mixin.yaml 失败: {}",
            paths.profile_mixin_file.display()
        )
    })?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "profile.mixin.reset",
            "existed": true,
        }));
    }

    println!("已重置 mixin.yaml。");
    println!("提示: 执行 `clash profile render` 使变更生效");
    Ok(())
}

// --- helpers ---

fn load_mixin_or_empty(path: &std::path::Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Mapping(serde_yaml::Mapping::new()));
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 mixin.yaml 失败: {}", path.display()))?;
    let val: Value = serde_yaml::from_str(&content)
        .with_context(|| format!("解析 mixin.yaml 失败: {}", path.display()))?;
    Ok(val)
}

fn save_mixin(path: &std::path::Path, root: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let content = serde_yaml::to_string(root).context("序列化 mixin.yaml 失败")?;
    fs::write(path, content).with_context(|| format!("写入 mixin.yaml 失败: {}", path.display()))
}

fn parse_yaml_value(raw: &str) -> Value {
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "~" => Value::Null,
        _ => {
            if let Ok(n) = raw.parse::<i64>() {
                return Value::Number(serde_yaml::Number::from(n));
            }
            if let Ok(f) = raw.parse::<f64>() {
                if let Some(n) = serde_yaml::Number::from(f).as_f64() {
                    let _ = n;
                    return Value::Number(serde_yaml::Number::from(f));
                }
            }
            Value::String(raw.to_string())
        }
    }
}

fn set_nested_key(root: &mut Value, dotted_key: &str, value: Value) -> Result<()> {
    let segments: Vec<&str> = dotted_key.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        bail!("无效的 key 路径: {}", dotted_key);
    }

    let mut current = root;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            let mapping = ensure_mapping(current);
            mapping.insert(Value::String(seg.to_string()), value);
            return Ok(());
        }
        let mapping = ensure_mapping(current);
        let key = Value::String(seg.to_string());
        if !mapping.contains_key(&key) {
            mapping.insert(key.clone(), Value::Mapping(serde_yaml::Mapping::new()));
        }
        current = mapping.get_mut(&key).unwrap();
    }
    Ok(())
}

fn unset_nested_key(root: &mut Value, dotted_key: &str) -> bool {
    let segments: Vec<&str> = dotted_key.split('.').collect();
    if segments.is_empty() {
        return false;
    }
    unset_recursive(root, &segments)
}

fn unset_recursive(current: &mut Value, segments: &[&str]) -> bool {
    if segments.is_empty() {
        return false;
    }
    let mapping = match current.as_mapping_mut() {
        Some(m) => m,
        None => return false,
    };
    let key = Value::String(segments[0].to_string());

    if segments.len() == 1 {
        return mapping.remove(&key).is_some();
    }

    if let Some(child) = mapping.get_mut(&key) {
        let removed = unset_recursive(child, &segments[1..]);
        // 如果子 mapping 空了，清理掉
        if removed {
            if let Some(m) = child.as_mapping() {
                if m.is_empty() {
                    mapping.remove(&key);
                }
            }
        }
        removed
    } else {
        false
    }
}

fn ensure_mapping(val: &mut Value) -> &mut serde_yaml::Mapping {
    if !val.is_mapping() {
        *val = Value::Mapping(serde_yaml::Mapping::new());
    }
    val.as_mapping_mut().expect("mapping")
}

fn yaml_to_json(val: &Value) -> serde_json::Value {
    match val {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(serde_json::Number::from(i))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Sequence(seq) => serde_json::Value::Array(seq.iter().map(yaml_to_json).collect()),
        Value::Mapping(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter_map(|(k, v)| k.as_str().map(|s| (s.to_string(), yaml_to_json(v))))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

fn mixin_command_requires_write(command: &MixinCommand) -> bool {
    matches!(
        command,
        MixinCommand::Set(_) | MixinCommand::Unset(_) | MixinCommand::Reset
    )
}

fn run_mixin_with_sudo(command: &MixinCommand) -> Result<()> {
    let cli_args = mixin_command_to_cli_args(command);
    let status = auto_sudo::run_with_sudo(is_json_mode(), |cmd| {
        cmd.args(&cli_args);
        Ok(())
    })?;
    if status.success() {
        return Ok(());
    }
    bail!("sudo 授权未通过或命令执行失败，请手动使用 sudo 重试");
}

fn mixin_command_to_cli_args(command: &MixinCommand) -> Vec<String> {
    let mut args = vec!["profile".to_string(), "mixin".to_string()];
    match command {
        MixinCommand::Show => args.push("show".to_string()),
        MixinCommand::Set(v) => {
            args.push("set".to_string());
            args.push("--key".to_string());
            args.push(v.key.clone());
            args.push("--value".to_string());
            args.push(v.value.clone());
        }
        MixinCommand::Unset(v) => {
            args.push("unset".to_string());
            args.push("--key".to_string());
            args.push(v.key.clone());
        }
        MixinCommand::Reset => args.push("reset".to_string()),
    }
    args
}
