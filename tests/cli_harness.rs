//! Machine Contract v0 + 关键状态不变量。
//! 不执行任何真实 TUN 命令。

mod common;

use common::{
    assert_machine_err, assert_machine_ok, binary_path, fixture_text, run_with_home, temp_home,
    write_profile,
};
use std::fs;
use std::process::Command;

#[test]
fn help_exposes_machine_not_legacy_json_or_profile_alias() {
    let output = Command::new(binary_path()).arg("--help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("--machine"));
    assert!(!text.contains("--json"));
    assert!(!text.contains("--text"));
    assert!(text.contains("  sub "));
    assert!(!text.contains("aliases: profile"));
}

#[test]
fn machine_sub_list_has_stable_read_envelope() {
    let home = temp_home("machine_sub_list");
    let output = run_with_home(&home, &["--machine", "sub", "list"]);
    let value = assert_machine_ok(&output, "sub.list");
    assert_eq!(value["status"], "success");
    assert_eq!(value["effect"]["state_changed"], false);
    assert_eq!(value["effect"]["verified"], true);
    assert_eq!(value["data"]["active"], serde_json::Value::Null);
    assert_eq!(value["data"]["subscriptions"], serde_json::json!([]));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn piped_human_output_does_not_silently_become_machine_json() {
    let home = temp_home("human_pipe");
    let output = run_with_home(&home, &["sub", "list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("暂无"));
    assert!(serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_parse_error_has_typed_code() {
    let home = temp_home("parse_error");
    let output = run_with_home(&home, &["--machine", "sub", "does-not-exist"]);
    let value = assert_machine_err(&output, "cli.parse", "CLI_ARGUMENT_INVALID");
    assert_eq!(value["effect"]["state_changed"], false);
    assert_eq!(value["error"]["retryable"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_missing_subscription_is_typed_and_non_mutating() {
    let home = temp_home("missing_subscription");
    let output = run_with_home(&home, &["--machine", "sub", "fetch", "--name", "missing"]);
    let value = assert_machine_err(&output, "sub.fetch", "PROFILE_NOT_FOUND");
    assert_eq!(value["effect"]["state_changed"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_setup_is_rejected_before_any_orchestration() {
    let home = temp_home("setup_rejected");
    let output = run_with_home(
        &home,
        &[
            "--machine",
            "setup",
            "init",
            "--sub-url",
            "file:///definitely/not-used.yaml",
            "--no-tun",
        ],
    );
    assert_machine_err(&output, "setup.init", "UNSUPPORTED_MACHINE_ACTION");
    assert!(!home.join("core").exists());
    assert!(!home.join("profiles").exists());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_follow_log_is_rejected_as_streaming_action() {
    let home = temp_home("follow_rejected");
    let output = run_with_home(&home, &["--machine", "service", "log", "--follow"]);
    assert_machine_err(&output, "service.log", "UNSUPPORTED_MACHINE_ACTION");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_env_off_is_read_only_data_not_raw_stdout() {
    let home = temp_home("env_off_machine");
    let output = run_with_home(&home, &["--machine", "env", "off"]);
    let value = assert_machine_ok(&output, "env.off");
    assert_eq!(value["effect"]["state_changed"], false);
    let script = value["data"]["script"].as_str().unwrap();
    assert!(script.contains("unset http_proxy"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn human_env_off_remains_directly_evalable_script() {
    let home = temp_home("env_off_human");
    let output = run_with_home(&home, &["env", "off"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("unset http_proxy"));
    assert!(!stdout.contains("clash.machine/v0"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn successful_mutation_reports_changed_but_unverified() {
    let home = temp_home("sub_add_effect");
    let output = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "add",
            "--name",
            "main",
            "--url",
            "file:///unused",
        ],
    );
    let value = assert_machine_ok(&output, "sub.add");
    assert_eq!(value["effect"]["state_changed"], true);
    assert_eq!(value["effect"]["verified"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn skipped_fetch_reports_no_state_change() {
    let home = temp_home("fetch_skip");
    let origin = home.join("origin.yaml");
    fs::write(&origin, "proxies: []\nrules:\n  - MATCH,DIRECT\n").unwrap();
    let url = format!("file://{}", origin.display());
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "add", "--name", "main", "--url", &url],
        ),
        "sub.add",
    );
    let first = run_with_home(&home, &["--machine", "sub", "fetch", "--name", "main"]);
    let first_value = assert_machine_ok(&first, "sub.fetch");
    assert_ne!(first_value["data"]["skipped"], true);
    let output = run_with_home(&home, &["--machine", "sub", "fetch", "--name", "main"]);
    let value = assert_machine_ok(&output, "sub.fetch");
    assert_eq!(value["data"]["skipped"], true);
    assert_eq!(value["effect"]["state_changed"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn bad_upstream_never_overwrites_good_subscription_or_runtime() {
    let home = temp_home("bad_upstream");
    let origin = home.join("origin.yaml");
    let good = "proxies: []\nrules:\n  - MATCH,DIRECT\nmode: rule\n";
    fs::write(&origin, good).unwrap();
    let url = format!("file://{}", origin.display());
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "add", "--name", "main", "--url", &url],
        ),
        "sub.add",
    );
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "fetch", "--name", "main", "--force"],
        ),
        "sub.fetch",
    );
    assert_machine_ok(
        &run_with_home(&home, &["--machine", "sub", "render", "--name", "main"]),
        "sub.render",
    );
    let profile_path = home.join("profiles/main.yaml");
    let runtime_path = home.join("runtime/config.yaml");
    let before_profile = fs::read(&profile_path).unwrap();
    let before_runtime = fs::read(&runtime_path).unwrap();
    fs::write(&origin, "upstream temporarily unavailable\n").unwrap();
    let output = run_with_home(
        &home,
        &["--machine", "sub", "fetch", "--name", "main", "--force"],
    );
    assert_machine_err(&output, "sub.fetch", "PROFILE_INVALID");
    assert_eq!(fs::read(&profile_path).unwrap(), before_profile);
    assert_eq!(fs::read(&runtime_path).unwrap(), before_runtime);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_use_is_rejected_because_it_would_split_active_from_runtime() {
    let home = temp_home("machine_use_rejected");
    let output = run_with_home(&home, &["--machine", "sub", "use", "--name", "main"]);
    assert_machine_err(&output, "sub.use", "UNSUPPORTED_MACHINE_ACTION");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_render_requires_fetched_subscription() {
    let home = temp_home("render_not_ready");
    let origin = home.join("origin.yaml");
    fs::write(&origin, "proxies: []\nrules: []\n").unwrap();
    let url = format!("file://{}", origin.display());
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "add", "--name", "main", "--url", &url],
        ),
        "sub.add",
    );
    let output = run_with_home(&home, &["--machine", "sub", "render", "--name", "main"]);
    assert_machine_err(&output, "sub.render", "PROFILE_NOT_READY");
    let listed = assert_machine_ok(
        &run_with_home(&home, &["--machine", "sub", "list"]),
        "sub.list",
    );
    assert!(listed["data"]["active"].is_null());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_render_commits_runtime_and_active_together() {
    let home = temp_home("render_commits_active");
    let origin = home.join("origin.yaml");
    fs::write(
        &origin,
        "proxies: []\nrules:\n  - MATCH,DIRECT\nmode: rule\n",
    )
    .unwrap();
    let url = format!("file://{}", origin.display());
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "add", "--name", "main", "--url", &url],
        ),
        "sub.add",
    );
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "fetch", "--name", "main", "--force"],
        ),
        "sub.fetch",
    );
    assert_machine_ok(
        &run_with_home(&home, &["--machine", "sub", "render", "--name", "main"]),
        "sub.render",
    );
    let listed = assert_machine_ok(
        &run_with_home(&home, &["--machine", "sub", "list"]),
        "sub.list",
    );
    assert_eq!(listed["data"]["active"], "main");
    assert!(home.join("runtime/config.yaml").is_file());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn duplicate_subscription_has_stable_error_code() {
    let home = temp_home("duplicate_sub");
    let first = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "add",
            "--name",
            "main",
            "--url",
            "file:///unused",
        ],
    );
    assert_machine_ok(&first, "sub.add");
    let second = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "add",
            "--name",
            "main",
            "--url",
            "file:///other",
        ],
    );
    assert_machine_err(&second, "sub.add", "PROFILE_ALREADY_EXISTS");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_cannot_remove_active_runtime_source() {
    let home = temp_home("remove_active");
    let origin = home.join("origin.yaml");
    fs::write(&origin, "proxies: []\nrules: []\n").unwrap();
    let url = format!("file://{}", origin.display());
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "add", "--name", "main", "--url", &url],
        ),
        "sub.add",
    );
    assert_machine_ok(
        &run_with_home(
            &home,
            &["--machine", "sub", "fetch", "--name", "main", "--force"],
        ),
        "sub.fetch",
    );
    assert_machine_ok(
        &run_with_home(&home, &["--machine", "sub", "render", "--name", "main"]),
        "sub.render",
    );
    let remove = run_with_home(&home, &["--machine", "sub", "remove", "--name", "main"]);
    assert_machine_err(&remove, "sub.remove", "STATE_CONFLICT");
    assert!(home.join("profiles/main.yaml").is_file());
    assert!(home.join("runtime/config.yaml").is_file());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_rejects_composite_subscription_shortcuts() {
    let home = temp_home("composite_sub");
    let update = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "update",
            "--name",
            "main",
            "--no-restart",
        ],
    );
    assert_machine_err(&update, "sub.update", "UNSUPPORTED_MACHINE_ACTION");
    let add = run_with_home(
        &home,
        &[
            "--machine",
            "sub",
            "add",
            "--name",
            "main",
            "--url",
            "file:///unused",
            "--fetch",
        ],
    );
    assert_machine_err(&add, "sub.add", "UNSUPPORTED_MACHINE_ACTION");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn api_failure_is_network_error_and_keeps_canonical_action() {
    let home = temp_home("api_failure");
    let output = run_with_home(
        &home,
        &[
            "--machine",
            "proxy",
            "list",
            "--controller",
            "127.0.0.1:1",
            "--timeout-secs",
            "1",
        ],
    );
    assert_machine_err(&output, "proxy.list", "NETWORK_ERROR");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn core_version_is_verified_read() {
    let home = temp_home("core_version");
    let output = run_with_home(&home, &["--machine", "core", "version"]);
    let value = assert_machine_ok(&output, "core.version");
    assert_eq!(value["data"]["installed"], false);
    assert_eq!(value["effect"]["state_changed"], false);
    assert_eq!(value["effect"]["verified"], true);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn contract_is_self_describing() {
    let home = temp_home("contract_describe");
    let output = run_with_home(&home, &["--machine", "contract"]);
    let value = assert_machine_ok(&output, "contract.describe");
    assert_eq!(value["data"]["version"], "clash.machine/v0");
    let actions = value["data"]["actions"].as_array().unwrap();
    assert!(actions.iter().any(|item| item["name"] == "sub.list"));
    assert!(actions.iter().any(|item| item["name"] == "system.on"));
    let human_only = value["data"]["human_only"].as_array().unwrap();
    assert!(human_only.iter().any(|item| item == "setup.init"));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_mutation_requiring_target_rejects_implicit_active() {
    let home = temp_home("explicit_target");
    write_profile(&home, &fixture_text("subscription-no-tun.yaml"));
    let output = run_with_home(&home, &["--machine", "sub", "render"]);
    assert_machine_err(&output, "sub.render", "EXPLICIT_INPUT_REQUIRED");
    assert!(!home.join("runtime/config.yaml").exists());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn hidden_human_proxy_state_commands_are_not_machine_capabilities() {
    let home = temp_home("human_proxy_only");
    let output = run_with_home(&home, &["--machine", "proxy", "status"]);
    assert_machine_err(&output, "proxy.status", "UNSUPPORTED_MACHINE_ACTION");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_env_on_requires_rendered_runtime_instead_of_guessing_ports() {
    let home = temp_home("env_requires_runtime");
    let output = run_with_home(&home, &["--machine", "env", "on"]);
    let value = assert_machine_err(&output, "env.on", "RUNTIME_CONFIG_REQUIRED");
    assert_eq!(value["effect"]["state_changed"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_proxy_list_requires_explicit_or_runtime_controller() {
    let home = temp_home("controller_required");
    let output = run_with_home(&home, &["--machine", "proxy", "list"]);
    let value = assert_machine_err(&output, "proxy.list", "EXPLICIT_INPUT_REQUIRED");
    assert_eq!(value["effect"]["state_changed"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_ui_url_requires_runtime_instead_of_guessing_9090() {
    let home = temp_home("ui_url_runtime");
    let output = run_with_home(&home, &["--machine", "ui", "url"]);
    assert_machine_err(&output, "ui.url", "RUNTIME_CONFIG_REQUIRED");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn machine_validate_requires_explicit_subscription() {
    let home = temp_home("validate_explicit");
    let output = run_with_home(&home, &["--machine", "sub", "validate"]);
    assert_machine_err(&output, "sub.validate", "EXPLICIT_INPUT_REQUIRED");
    let _ = fs::remove_dir_all(&home);
}
