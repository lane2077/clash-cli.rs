//! 库级 harness：直接调用 `clash_cli::harness` 纯函数，不启动二进制、不要求 Linux。

use clash_cli::harness;
use serde_yaml::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn parse_yaml(input: &str) -> Value {
    serde_yaml::from_str(input).expect("解析 YAML 失败")
}

fn yaml_bool(root: &Value, keys: &[&str]) -> Option<bool> {
    let mut current = root;
    for key in keys {
        current = current.get(*key)?;
    }
    current.as_bool()
}

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("读取 {} 失败: {err}", path.display()))
}

fn temp_home(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_nanos())
        .unwrap_or(0);
    path.push(format!(
        "clash_cli_api_{prefix}_{}_{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&path).expect("创建临时目录失败");
    path
}

#[test]
fn launchd_plist_identity_matches_core_link() {
    let core = PathBuf::from("/Users/me/.config/clash-cli/core/mihomo");
    let binary = harness::resolve_service_binary(None, &core);
    let plist = harness::build_launchd_plist(
        &binary,
        PathBuf::from("/Users/me/.config/clash-cli/runtime/config.yaml").as_path(),
        PathBuf::from("/Users/me/.config/clash-cli/runtime").as_path(),
        &harness::launchd_label("clash-mihomo"),
    );
    assert!(plist.contains("/Users/me/.config/clash-cli/core/mihomo"));
    assert!(plist.contains("-d"));
    assert!(plist.contains("-f"));
    assert!(!plist.contains("/usr/local/bin/mihomo"));
    let cfg = harness::launchd_plist_config_path(&plist).expect("应解析出 -f 配置路径");
    assert_eq!(
        cfg,
        PathBuf::from("/Users/me/.config/clash-cli/runtime/config.yaml")
    );
}

#[test]
fn darwin_overlay_does_not_require_auto_redirect() {
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay_for(&mut overlay, true, "macos");
    let tun = overlay.get("tun").unwrap();
    assert_eq!(tun.get("enable").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(tun.get("auto-route").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        tun.get("auto-redirect").and_then(|v| v.as_bool()),
        Some(false)
    );
}

#[test]
fn overlay_render_keeps_tun_when_fixture_has_no_tun() {
    let subscription = parse_yaml(&fixture("subscription-no-tun.yaml"));
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay_for(&mut overlay, true, "linux");
    let rendered = harness::merge_subscription_overlay(subscription, Some(&overlay), false);
    assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
    assert_eq!(yaml_bool(&rendered, &["tun", "auto-redirect"]), Some(true));
    assert_eq!(yaml_bool(&rendered, &["dns", "enable"]), Some(true));
}

#[test]
fn overlay_render_overrides_subscription_tun_false() {
    let subscription = parse_yaml(&fixture("subscription-tun-disabled.yaml"));
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay_for(&mut overlay, true, "linux");
    let rendered = harness::merge_subscription_overlay(subscription, Some(&overlay), false);
    assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
    assert_eq!(yaml_bool(&rendered, &["tun", "auto-redirect"]), Some(true));
}

#[test]
fn overlay_tun_off_clears_enable_after_render() {
    let subscription = parse_yaml(&fixture("subscription-no-tun.yaml"));
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay(&mut overlay, true);
    harness::apply_tun_policy_overlay(&mut overlay, false);
    let rendered = harness::merge_subscription_overlay(subscription, Some(&overlay), false);
    assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(false));
}

#[test]
fn render_runtime_from_home_writes_overlay() {
    let home = temp_home("render_home");
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    fs::write(
        profiles.join("index.json"),
        r#"{"active":"main","profiles":[{"name":"main","url":"https://example.com/sub.yaml","file":"main.yaml","created_at":1,"updated_at":1}]}"#,
    )
    .unwrap();
    fs::write(
        profiles.join("main.yaml"),
        fixture("subscription-no-tun.yaml"),
    )
    .unwrap();
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay(&mut overlay, true);
    fs::write(
        profiles.join("mixin.yaml"),
        serde_yaml::to_string(&overlay).unwrap(),
    )
    .unwrap();

    let paths = clash_cli::paths::AppPaths {
        config_dir: home.clone(),
        state_file: home.join("proxy.state"),
        env_file: home.join("proxy.env"),
        profile_dir: profiles.clone(),
        profile_index_file: profiles.join("index.json"),
        profile_mixin_file: profiles.join("mixin.yaml"),
        core_dir: home.join("core"),
        core_versions_dir: home.join("core").join("versions"),
        core_current_link: home.join("core").join("mihomo"),
        core_meta_file: home.join("core").join("current.meta"),
        runtime_dir: home.join("runtime"),
        runtime_config_file: home.join("runtime").join("config.yaml"),
        runtime_tun_state_file: home.join("runtime").join("tun.state"),
    };
    harness::render_runtime_from_home(&paths).expect("渲染失败");
    let text = fs::read_to_string(&paths.runtime_config_file).unwrap();
    let rendered: Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
    harness::render_runtime_from_home(&paths).expect("二次渲染失败");
    let text = fs::read_to_string(&paths.runtime_config_file).unwrap();
    let again: Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(yaml_bool(&again, &["tun", "enable"]), Some(true));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn apply_subscription_render_without_restart_is_the_use_update_seam() {
    let home = temp_home("apply_sub");
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    fs::write(
        profiles.join("index.json"),
        r#"{"active":"main","profiles":[{"name":"main","url":"https://example.com/sub.yaml","file":"main.yaml","created_at":1,"updated_at":1}]}"#,
    )
    .unwrap();
    fs::write(
        profiles.join("main.yaml"),
        fixture("subscription-no-tun.yaml"),
    )
    .unwrap();
    let mut overlay = Value::Mapping(serde_yaml::Mapping::new());
    harness::apply_tun_policy_overlay_for(&mut overlay, true, "linux");
    fs::write(
        profiles.join("mixin.yaml"),
        serde_yaml::to_string(&overlay).unwrap(),
    )
    .unwrap();
    let paths = clash_cli::paths::AppPaths {
        config_dir: home.clone(),
        state_file: home.join("proxy.state"),
        env_file: home.join("proxy.env"),
        profile_dir: profiles.clone(),
        profile_index_file: profiles.join("index.json"),
        profile_mixin_file: profiles.join("mixin.yaml"),
        core_dir: home.join("core"),
        core_versions_dir: home.join("core").join("versions"),
        core_current_link: home.join("core").join("mihomo"),
        core_meta_file: home.join("core").join("current.meta"),
        runtime_dir: home.join("runtime"),
        runtime_config_file: home.join("runtime").join("config.yaml"),
        runtime_tun_state_file: home.join("runtime").join("tun.state"),
    };
    let first = harness::apply_subscription(
        &paths,
        harness::ApplySpec {
            name: Some("main".into()),
            fetch: false,
            render: true,
            restart: false,
            service_name: "clash-cli-harness-uninstalled".into(),
        },
    )
    .expect("第一次生效");
    assert_eq!(first.name, "main");
    assert!(!first.fetched);
    assert!(first.applied);
    assert!(!first.restarted);
    let rendered: Value =
        serde_yaml::from_str(&fs::read_to_string(&paths.runtime_config_file).unwrap()).unwrap();
    assert_eq!(yaml_bool(&rendered, &["tun", "enable"]), Some(true));
    let second = harness::apply_subscription(
        &paths,
        harness::ApplySpec {
            name: None,
            fetch: false,
            render: true,
            restart: false,
            service_name: "clash-cli-harness-uninstalled".into(),
        },
    )
    .expect("第二次生效不得冲掉 overlay");
    assert!(second.applied);
    let again: Value =
        serde_yaml::from_str(&fs::read_to_string(&paths.runtime_config_file).unwrap()).unwrap();
    assert_eq!(yaml_bool(&again, &["tun", "enable"]), Some(true));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn actual_ok_ignores_cli_tables() {
    assert!(harness::actual_tun_ok(true, true, true));
    assert!(!harness::actual_tun_ok(true, true, false));
}

#[test]
fn docker_note_never_mentions_include_exclude() {
    let note =
        harness::docker_bridge_dataplane_note(&["docker0".to_string()], true).expect("应有提示");
    let blob = format!("{}{:?}", note.message, note.suggestion);
    assert!(!blob.contains("include-interface"));
    assert!(!blob.contains("exclude-interface"));
}

#[test]
fn service_unit_execstart_matches_core_link() {
    let core = PathBuf::from("/etc/clash-cli/core/mihomo");
    let binary = harness::resolve_service_binary(None, &core);
    assert_eq!(binary, core);
    let unit = harness::build_unit_content(
        &binary,
        std::path::Path::new("/etc/clash-cli/runtime/config.yaml"),
        std::path::Path::new("/etc/clash-cli/runtime"),
        false,
        "clash-mihomo.service",
    );
    assert!(unit.contains("ExecStart=/etc/clash-cli/core/mihomo "));
    assert!(!unit.contains("/usr/local/bin/mihomo"));
}
