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
    for cmd in [
        "sub", "system", "mode", "env", "ui", "tun", "proxy", "profile", "api", "core", "service",
    ] {
        assert!(stdout.contains(cmd), "帮助信息缺少子命令: {cmd}");
    }
    assert!(
        stdout.contains("代理组") || stdout.contains("节点"),
        "proxy 帮助应说明代理组/节点: {stdout}"
    );
    assert!(
        !stdout.contains("管理终端代理"),
        "顶层 proxy 不应再表示终端环境变量: {stdout}"
    );
    assert!(
        !stdout.contains("inspect"),
        "帮助不应再提供 inspect 子命令: {stdout}"
    );
    assert!(
        !stdout.contains("AI 入口") && !stdout.contains("clash --json inspect"),
        "帮助不应把 inspect 当成 AI 入口: {stdout}"
    );
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

#[test]
fn help_should_contain_mixin_and_update() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("执行 --help 失败");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("update"), "帮助信息缺少 update 命令");
    assert!(stdout.contains("mixin"), "帮助信息缺少 mixin 关键字");
}

#[test]
fn json_mixin_show_should_return_empty_on_new_home() {
    let home = temp_home("mixin_show");
    fs::create_dir_all(&home).expect("创建测试目录失败");
    let output = run_with_home(&home, &["--json", "profile", "mixin", "show"]);
    assert!(output.status.success());

    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["exists"], false);
    assert_eq!(value["content"], serde_json::Value::Null);

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_mixin_set_and_show_should_persist() {
    let home = temp_home("mixin_set");
    fs::create_dir_all(&home).expect("创建测试目录失败");

    let output = run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "mixin",
            "set",
            "--key",
            "tun.enable",
            "--value",
            "true",
        ],
    );
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["action"], "profile.mixin.set");

    let output = run_with_home(&home, &["--json", "profile", "mixin", "show"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["exists"], true);
    let tun_enable = &value["content"]["tun"]["enable"];
    assert_eq!(*tun_enable, serde_json::Value::Bool(true));

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_mixin_reset_should_clear() {
    let home = temp_home("mixin_reset");
    fs::create_dir_all(&home).expect("创建测试目录失败");

    run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "mixin",
            "set",
            "--key",
            "dns.enable",
            "--value",
            "true",
        ],
    );

    let output = run_with_home(&home, &["--json", "profile", "mixin", "reset"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["existed"], true);

    let output = run_with_home(&home, &["--json", "profile", "mixin", "show"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&text).expect("输出不是合法 JSON");
    assert_eq!(value["exists"], false);

    let _ = fs::remove_dir_all(&home);
}

fn write_profile_without_tun(home: &Path, extra: &str) {
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).expect("创建 profiles 目录失败");
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
    fs::write(
        profiles.join("main.yaml"),
        format!("proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n{extra}"),
    )
    .expect("写入 profile 失败");
}

fn runtime_tun_enable(home: &Path) -> Option<bool> {
    let text = fs::read_to_string(home.join("runtime").join("config.yaml"))
        .expect("读取 runtime/config.yaml 失败");
    let root: serde_yaml::Value = serde_yaml::from_str(&text).expect("解析 runtime YAML 失败");
    root.get("tun")
        .and_then(|t| t.get("enable"))
        .and_then(|v| v.as_bool())
}

#[test]
fn profile_render_keeps_tun_overlay_after_subscription_omits_tun() {
    let home = temp_home("render_tun_overlay");
    write_profile_without_tun(&home, "");

    let mixin_out = run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "mixin",
            "set",
            "--key",
            "tun.enable",
            "--value",
            "true",
        ],
    );
    assert!(
        mixin_out.status.success(),
        "mixin set 失败: {}",
        String::from_utf8_lossy(&mixin_out.stderr)
    );

    let mixin_redirect = run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "mixin",
            "set",
            "--key",
            "tun.auto-redirect",
            "--value",
            "true",
        ],
    );
    assert!(mixin_redirect.status.success());

    let first_render = run_with_home(&home, &["--json", "profile", "render"]);
    assert!(
        first_render.status.success(),
        "第一次 render 失败: stdout={} stderr={}",
        String::from_utf8_lossy(&first_render.stdout),
        String::from_utf8_lossy(&first_render.stderr)
    );
    let first_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&first_render.stdout))
            .expect("第一次 render 不是 JSON");
    assert_eq!(first_json["ok"], true);
    assert_eq!(runtime_tun_enable(&home), Some(true));

    write_profile_without_tun(&home, "mode: rule\n");

    let second_render = run_with_home(&home, &["--json", "profile", "render"]);
    assert!(
        second_render.status.success(),
        "第二次 render 失败: stdout={} stderr={}",
        String::from_utf8_lossy(&second_render.stdout),
        String::from_utf8_lossy(&second_render.stderr)
    );
    let second_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&second_render.stdout))
            .expect("第二次 render 不是 JSON");
    assert_eq!(second_json["ok"], true);
    assert_eq!(runtime_tun_enable(&home), Some(true));

    let text = fs::read_to_string(home.join("runtime").join("config.yaml")).unwrap();
    let root: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(
        root.get("tun")
            .and_then(|t| t.get("auto-redirect"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    let _ = fs::remove_dir_all(&home);
}
