#![allow(dead_code)]

//! 集成测试共用：隔离 CLASH_CLI_HOME + 调用真实 `clash` 二进制。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_clash")
}

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn fixture_text(name: &str) -> String {
    let path = fixtures_dir().join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读取 fixture 失败 {}: {err}", path.display()))
}

pub fn temp_home(prefix: &str) -> PathBuf {
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
    fs::create_dir_all(&path).expect("创建隔离 home 失败");
    path
}

pub fn run_with_home(home: &Path, args: &[&str]) -> Output {
    Command::new(binary_path())
        .args(args)
        .env("CLASH_CLI_HOME", home)
        .env("CLASH_CLI_NO_AUTO_SUDO", "1")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .expect("命令执行失败")
}

pub fn parse_machine(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "stdout 不是 Machine Contract JSON: {err}\nstatus={:?}\nstdout={}\nstderr={}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

pub fn assert_machine_ok(output: &Output, action: &str) -> serde_json::Value {
    assert!(
        output.status.success(),
        "命令失败: status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value = parse_machine(output);
    assert_eq!(value["contract"], "clash.machine/v0");
    assert_eq!(value["ok"], true);
    assert_eq!(value["action"], action);
    assert!(value["error"].is_null());
    value
}

pub fn assert_machine_err(output: &Output, action: &str, code: &str) -> serde_json::Value {
    assert!(
        !output.status.success(),
        "期望失败却成功: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value = parse_machine(output);
    assert_eq!(value["contract"], "clash.machine/v0");
    assert_eq!(value["ok"], false);
    assert_eq!(value["status"], "failed");
    assert_eq!(value["action"], action);
    assert_eq!(value["error"]["code"], code);
    value
}

pub fn write_profile(home: &Path, yaml: &str) {
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).expect("创建 profiles 失败");
    let index = serde_json::json!({
        "active": "main",
        "profiles": [{
            "name": "main",
            "url": "https://example.com/sub.yaml",
            "file": "main.yaml",
            "created_at": 1,
            "updated_at": 1
        }]
    });
    fs::write(
        profiles.join("index.json"),
        serde_json::to_vec_pretty(&index).expect("序列化索引失败"),
    )
    .expect("写入索引失败");
    fs::write(profiles.join("main.yaml"), yaml).expect("写入订阅失败");
}

pub fn read_runtime_yaml(home: &Path) -> serde_yaml::Value {
    let path = home.join("runtime").join("config.yaml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("读取 {} 失败: {err}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|err| panic!("解析 runtime YAML 失败: {err}\n{text}"))
}
