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

pub(super) fn remove_tun_key(root: &mut Value, key: &str) {
    if let Some(tun) = root
        .as_mapping_mut()
        .and_then(|m| m.get_mut(Value::String("tun".to_string())))
        .and_then(|v| v.as_mapping_mut())
    {
        tun.remove(Value::String(key.to_string()));
    }
}

/// 将 tun on/off 的本地策略写入 overlay（mixin），而不是直接改 runtime YAML。
/// 仅在开启时填入产品默认值（已有字段不覆盖），关闭时只把 `tun.enable` 设为 false。
pub fn apply_tun_policy_overlay(root: &mut Value, enabled: bool) {
    apply_tun_policy_overlay_for(root, enabled, std::env::consts::OS)
}

pub fn apply_tun_policy_overlay_for(root: &mut Value, enabled: bool, os: &str) {
    set_bool_field(root, &["tun"], "enable", enabled);
    if !enabled {
        return;
    }

    set_default_bool_field(root, &["tun"], "auto-route", true);
    set_default_bool_field(root, &["tun"], "auto-detect-interface", true);
    if os == "macos" {
        // macOS 走 utun + auto-route，没有 Linux nft auto-redirect。
        set_default_bool_field(root, &["tun"], "auto-redirect", false);
    } else {
        set_default_bool_field(root, &["tun"], "auto-redirect", true);
        // 固定接口名，便于状态检查确认运行时接口确实已经创建。
        set_default_string_field(root, &["tun"], "device", "Meta");
        set_default_u16_field(
            root,
            &[],
            "redir-port",
            crate::constants::DEFAULT_REDIR_PORT,
        );
    }
    // strict-route 在部分发行版/路由环境下更容易触发 /1 路由写入失败，默认保守关闭。
    set_default_bool_field(root, &["tun"], "strict-route", false);
    set_default_string_field(root, &["tun"], "stack", "mixed");
    set_default_bool_field(root, &["dns"], "enable", true);
    set_bool_field(root, &[], "ipv6", false);
    set_bool_field(root, &["dns"], "ipv6", false);
    set_default_string_field(root, &["dns"], "enhanced-mode", "fake-ip");
    set_default_sequence_field(root, &["tun"], "dns-hijack", &["any:53".to_string()]);
    remove_tun_key(root, "include-interface");
    remove_tun_key(root, "exclude-interface");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tun_on_overlay_sets_enable_and_auto_redirect_defaults() {
        let mut root = Value::Mapping(Mapping::new());
        apply_tun_policy_overlay_for(&mut root, true, "linux");
        assert_eq!(bool_field(key_value(&root, "tun"), "enable"), Some(true));
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-redirect"),
            Some(true)
        );
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-route"),
            Some(true)
        );
        assert_eq!(
            string_field(key_value(&root, "tun"), "device").as_deref(),
            Some("Meta")
        );
        assert!(
            key_value(&root, "tun")
                .and_then(|tun| tun.as_mapping())
                .and_then(|tun| tun.get(Value::String("exclude-uid".to_string())))
                .is_none(),
            "不应根据容器进程自动排除整个宿主 UID"
        );
        assert_eq!(bool_field(key_value(&root, "dns"), "enable"), Some(true));
        assert_eq!(
            string_field(key_value(&root, "dns"), "enhanced-mode").as_deref(),
            Some("fake-ip")
        );
    }

    #[test]
    fn tun_off_overlay_only_clears_enable() {
        let mut root = Value::Mapping(Mapping::new());
        apply_tun_policy_overlay_for(&mut root, true, "linux");
        apply_tun_policy_overlay_for(&mut root, false, "linux");
        assert_eq!(bool_field(key_value(&root, "tun"), "enable"), Some(false));
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-redirect"),
            Some(true)
        );
    }

    #[test]
    fn tun_on_overlay_does_not_override_existing_auto_redirect() {
        let mut root = Value::Mapping(Mapping::new());
        set_bool_field(&mut root, &["tun"], "auto-redirect", false);
        apply_tun_policy_overlay_for(&mut root, true, "linux");
        assert_eq!(bool_field(key_value(&root, "tun"), "enable"), Some(true));
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-redirect"),
            Some(false)
        );
    }

    #[test]
    fn darwin_tun_on_uses_auto_route_not_auto_redirect() {
        let mut root = Value::Mapping(Mapping::new());
        apply_tun_policy_overlay_for(&mut root, true, "macos");
        assert_eq!(bool_field(key_value(&root, "tun"), "enable"), Some(true));
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-route"),
            Some(true)
        );
        assert_eq!(
            bool_field(key_value(&root, "tun"), "auto-redirect"),
            Some(false)
        );
        assert!(u16_field(Some(&root), "redir-port").is_none());
    }
}
