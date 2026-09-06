//! 轻量 CLI 集成测试：只覆盖不触碰真实 TUN / 服务写操作的路径。

mod common;

use common::{assert_machine_ok, binary_path, run_with_home, temp_home};
use std::fs;
use std::process::Command;

#[test]
fn help_contains_only_canonical_top_level_commands() {
    let output = Command::new(binary_path()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in [
        "sub", "proxy", "system", "tun", "env", "mode", "ui", "core", "service", "api", "setup",
        "update",
    ] {
        assert!(stdout.contains(cmd), "缺少命令 {cmd}: {stdout}");
    }
    assert!(!stdout.contains("aliases: profile"));
}

#[test]
fn removed_profile_alias_is_rejected() {
    let output = Command::new(binary_path())
        .args(["profile", "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn removed_proxy_env_alias_is_rejected() {
    let output = Command::new(binary_path())
        .args(["proxy", "env", "off"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn mixin_machine_roundtrip_uses_sub_namespace() {
    let home = temp_home("mixin_roundtrip");
    let set = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "mixin",
            "set",
            "--key",
            "mode",
            "--value",
            "rule",
        ],
    );
    let set_value = assert_machine_ok(&set, "sub.mixin.set");
    assert_eq!(set_value["effect"]["state_changed"], true);
    let show = run_with_home(&home, &["--machine", "sub", "mixin", "show"]);
    let show_value = assert_machine_ok(&show, "sub.mixin.show");
    assert_eq!(show_value["data"]["exists"], true);
    assert_eq!(show_value["data"]["content"]["mode"], "rule");
    assert_eq!(show_value["effect"]["verified"], true);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn ui_status_is_safe_verified_read() {
    let home = temp_home("ui_status");
    let output = run_with_home(&home, &["--machine", "ui", "status"]);
    let value = assert_machine_ok(&output, "ui.status");
    assert_eq!(value["data"]["installed"], false);
    assert_eq!(value["effect"]["state_changed"], false);
    assert_eq!(value["effect"]["verified"], true);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn system_status_is_read_only_machine_action() {
    let home = temp_home("system_status");
    let output = run_with_home(&home, &["--machine", "system", "status"]);
    let value = assert_machine_ok(&output, "system.status");
    assert_eq!(value["effect"]["state_changed"], false);
    assert_eq!(value["effect"]["verified"], true);
    assert_eq!(value["data"]["committed"]["managed"], false);
    assert!(value["data"]["observed"]["matches_committed"].is_null());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_mode_is_not_implicitly_enabled_by_environment() {
    let home = temp_home("machine_env_ignored");
    let output = Command::new(binary_path())
        .args(["core", "version"])
        .env("CLASH_CLI_HOME", &home)
        .env("CLASH_CLI_MACHINE", "true")
        .env("CLASH_CLI_NO_AUTO_SUDO", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("clash.machine/v0"));
    let _ = fs::remove_dir_all(&home);
}
