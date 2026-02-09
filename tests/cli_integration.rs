use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_clash")
}

fn temp_home(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_nanos())
        .unwrap_or(0);
    path.push(format!(
        "clash_cli_test_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ));
    path
}

fn run_with_home(home: &Path, args: &[&str]) -> Output {
    Command::new(binary_path())
        .args(args)
        .env("CLASH_CLI_HOME", home)
        .output()
        .expect("命令执行失败")
}

#[test]
fn help_should_contain_main_commands() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("执行 --help 失败");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for cmd in ["proxy", "core", "service", "tun", "profile", "api"] {
        assert!(stdout.contains(cmd), "帮助信息缺少子命令: {cmd}");
    }
}

#[test]
fn json_profile_list_should_return_empty_index_on_new_home() {
    let home = temp_home("profile_list");
    fs::create_dir_all(&home).expect("创建测试目录失败");
    let output = run_with_home(&home, &["--json", "profile", "list"]);
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["active"], serde_json::Value::Null);
    assert_eq!(
        value["profiles"]
            .as_array()
            .expect("profiles 不是数组")
            .len(),
        0
    );

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_proxy_env_off_should_return_unset_script() {
    let home = temp_home("proxy_env_off");
    fs::create_dir_all(&home).expect("创建测试目录失败");
    let output = run_with_home(&home, &["--json", "proxy", "env", "off"]);
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["action"], "proxy.env.off");
    let script = value["script"].as_str().expect("script 不是字符串");
    assert!(script.contains("unset http_proxy"));
    assert!(script.contains("unset HTTPS_PROXY"));

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_core_version_should_mark_uninstalled_on_new_home() {
    let home = temp_home("core_version");
    fs::create_dir_all(&home).expect("创建测试目录失败");
    let output = run_with_home(&home, &["--json", "core", "version"]);
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["action"], "core.version");
    assert_eq!(value["installed"], false);
    assert_eq!(value["version"], serde_json::Value::Null);

    let _ = fs::remove_dir_all(&home);
}
