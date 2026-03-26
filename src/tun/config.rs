use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Number, Value};

pub(super) fn key_value<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    root.as_mapping()
        .and_then(|m| m.get(Value::String(key.to_string())))
}

pub(super) fn bool_field(root: Option<&Value>, key: &str) -> Option<bool> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| v.as_bool())
    })
}

pub(super) fn string_field(root: Option<&Value>, key: &str) -> Option<String> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
    })
}

pub(super) fn u16_field(root: Option<&Value>, key: &str) -> Option<u16> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| {
                if let Some(i) = v.as_i64() {
                    return u16::try_from(i).ok();
                }
                if let Some(s) = v.as_str() {
                    return s.parse::<u16>().ok();
                }
                None
            })
    })
}

pub(super) fn load_or_init_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        fs::write(path, "{}\n")
            .with_context(|| format!("初始化配置文件失败: {}", path.display()))?;
    }
    load_existing_config(path)
}

pub(super) fn load_existing_config(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?;
    let root: Value = serde_yaml::from_str(&content)
        .with_context(|| format!("解析 YAML 失败: {}", path.display()))?;
    if root.is_mapping() {
        Ok(root)
    } else {
        Ok(Value::Mapping(Mapping::new()))
    }
}

pub(super) fn save_config(path: &Path, root: &Value) -> Result<()> {
    let text = serde_yaml::to_string(root).context("序列化 YAML 失败")?;
    fs::write(path, text).with_context(|| format!("写入配置失败: {}", path.display()))
}

pub(super) fn set_bool_field(root: &mut Value, path_keys: &[&str], key: &str, value: bool) {
    ensure_mapping_path(root, path_keys).insert(Value::String(key.to_string()), Value::Bool(value));
}

pub(super) fn set_default_bool_field(root: &mut Value, path_keys: &[&str], key: &str, value: bool) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::Bool(value));
    }
}

pub(super) fn set_default_string_field(
    root: &mut Value,
    path_keys: &[&str],
    key: &str,
    value: &str,
) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::String(value.to_string()));
    }
}

pub(super) fn set_default_u16_field(root: &mut Value, path_keys: &[&str], key: &str, value: u16) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::Number(Number::from(value as i64)));
    }
}

pub(super) fn set_default_sequence_field(
    root: &mut Value,
    path_keys: &[&str],
    key: &str,
    values: &[String],
) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        let seq: Vec<Value> = values.iter().map(|v| Value::String(v.clone())).collect();
        map.insert(key_v, Value::Sequence(seq));
    }
}

pub(super) fn set_u32_sequence_field(
    root: &mut Value,
    path_keys: &[&str],
    key: &str,
    values: &[u32],
) {
    let seq: Vec<Value> = values
        .iter()
        .map(|v| Value::Number(Number::from(*v as i64)))
        .collect();
    ensure_mapping_path(root, path_keys)
        .insert(Value::String(key.to_string()), Value::Sequence(seq));
}

pub(super) fn remove_tun_key(root: &mut Value, key: &str) {
    if let Some(tun) = root
        .as_mapping_mut()
        .and_then(|m| m.get_mut(Value::String("tun".to_string())))
        .and_then(|v| v.as_mapping_mut())
    {
        tun.remove(Value::String(key.to_string()));
    }
}

fn ensure_mapping_path<'a>(root: &'a mut Value, path_keys: &[&str]) -> &'a mut Mapping {
    if !root.is_mapping() {
        *root = Value::Mapping(Mapping::new());
    }
    let mut cursor = root;
    for key in path_keys {
        let map = cursor.as_mapping_mut().expect("mapping");
        let key_v = Value::String((*key).to_string());
        if !map.contains_key(&key_v) {
            map.insert(key_v.clone(), Value::Mapping(Mapping::new()));
        }
        let child = map.get_mut(&key_v).expect("child");
        if !child.is_mapping() {
            *child = Value::Mapping(Mapping::new());
        }
        cursor = child;
    }
    cursor.as_mapping_mut().expect("mapping")
}
