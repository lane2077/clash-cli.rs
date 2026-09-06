//! CLI 调试 harness：覆盖 JSON 契约、失败形态，以及 macOS 上不再 Linux-only 的命令。

mod common;

use common::{
    assert_json_err, assert_json_ok, fixture_text, read_runtime_yaml, run_with_home, temp_home,
    write_profile, yaml_bool,
};
use std::fs;

#[test]
fn json_error_shape_when_profile_missing() {
    let home = temp_home("json_err_missing_profile");
    let output = run_with_home(&home, &["--json", "profile", "use", "--name", "no-such"]);
    let value = assert_json_err(&output);
    let error = value["error"].as_str().unwrap();
    assert!(
        error.contains("profile 不存在"),
        "错误文案应指向缺失 profile: {error}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_error_shape_when_render_without_profile() {
    let home = temp_home("json_err_render_empty");
    let output = run_with_home(&home, &["--json", "profile", "render"]);
    let value = assert_json_err(&output);
    let error = value["error"].as_str().unwrap();
    assert!(
        error.contains("active profile") || error.contains("profile"),
        "render 无 profile 时应给出可读错误: {error}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn macos_tun_on_is_not_linux_only() {
    if cfg!(target_os = "linux") {
        return;
    }
    let home = temp_home("tun_on_macos");
    let output = run_with_home(&home, &["tun", "on", "--no-restart"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("当前仅支持 Linux 平台"),
        "tun on 不应再报 Linux-only: {combined}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn macos_tun_off_is_not_linux_only() {
    if cfg!(target_os = "linux") {
        return;
    }
    let home = temp_home("tun_off_macos");
    let output = run_with_home(&home, &["tun", "off", "--no-restart"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("当前仅支持 Linux 平台"),
        "tun off 不应再报 Linux-only: {combined}"
    );
    assert!(
        !combined.contains("未检测到 iptables"),
        "tun off 不应在 macOS 上要求 iptables: {combined}"
    );
    assert!(
        output.status.success(),
        "macOS tun off 只写 overlay，不应要求 root: {combined}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn macos_service_status_is_not_linux_only() {
    if cfg!(target_os = "linux") {
        return;
    }
    let home = temp_home("service_macos");
    let output = run_with_home(&home, &["service", "status"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("当前仅支持 Linux 平台"),
        "service status 不应再报 Linux-only: {combined}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn proxy_start_status_env_json_roundtrip() {
    let home = temp_home("proxy_roundtrip");
    let start = run_with_home(
        &home,
        &[
            "--json",
            "proxy",
            "start",
            "--host",
            "127.0.0.1",
            "--http-port",
            "17890",
            "--socks-port",
            "17891",
        ],
    );
    let start_json = assert_json_ok(&start);
    assert_eq!(start_json["action"], "proxy.start");

    let status = run_with_home(&home, &["--json", "proxy", "status"]);
    let status_json = assert_json_ok(&status);
    assert_eq!(status_json["action"], "proxy.status");

    let env_on = run_with_home(&home, &["--json", "proxy", "env", "on"]);
    let env_json = assert_json_ok(&env_on);
    let script = env_json["script"].as_str().expect("缺少 script");
    assert!(
        script.contains("17890"),
        "export 脚本应包含 http 端口: {script}"
    );
    assert!(
        script.contains("17891"),
        "export 脚本应包含 socks 端口: {script}"
    );

    let stop = run_with_home(&home, &["--json", "proxy", "stop"]);
    assert_json_ok(&stop);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn profile_validate_and_render_from_fixture() {
    let home = temp_home("validate_render");
    write_profile(&home, &fixture_text("subscription-no-tun.yaml"));

    let validate = run_with_home(&home, &["--json", "profile", "validate"]);
    let validate_json = assert_json_ok(&validate);
    assert_eq!(validate_json["action"], "profile.validate");

    let mixin = run_with_home(
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
    assert_json_ok(&mixin);

    let render = run_with_home(&home, &["--json", "profile", "render"]);
    let render_json = assert_json_ok(&render);
    assert_eq!(render_json["action"], "profile.render");
    assert_eq!(render_json["ok"], true);

    let runtime = read_runtime_yaml(&home);
    assert_eq!(yaml_bool(&runtime, &["tun", "enable"]), Some(true));
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn profile_use_apply_without_systemctl_does_not_query_execstart() {
    let home = temp_home("use_apply_no_sysctl");
    write_profile(&home, &fixture_text("subscription-no-tun.yaml"));
    let output = run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "use",
            "--name",
            "main",
            "--apply",
            "--service-name",
            "clash-cli-harness-uninstalled",
        ],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("读取 service ExecStart 失败"),
        "无 systemctl 时 use --apply 不得硬失败: {combined}"
    );
    if cfg!(not(target_os = "linux")) {
        assert!(
            !combined.contains("ExecStart"),
            "Darwin use --apply 不应查询 systemd ExecStart: {combined}"
        );
        assert!(
            !combined.to_lowercase().contains("systemctl"),
            "Darwin use --apply 失败应走 launchd，而不是 systemctl: {combined}"
        );
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_profile_use_apply_emits_single_object() {
    let home = temp_home("use_apply_json");
    write_profile(&home, &fixture_text("subscription-no-tun.yaml"));
    let output = run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "use",
            "--name",
            "main",
            "--apply",
            "--no-restart",
        ],
    );
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "profile.use");
    assert_eq!(value["applied"], true);
    assert_eq!(value["restarted"], false);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout 必须是单个 JSON 对象");
    assert_eq!(parsed["action"], "profile.use");
    let runtime = read_runtime_yaml(&home);
    assert_eq!(
        runtime.get("mode").and_then(|v| v.as_str()),
        Some("rule"),
        "use --apply 应把订阅写进 runtime: {runtime:?}"
    );
    assert!(
        runtime.get("rules").is_some(),
        "use --apply 的 runtime 应含订阅 rules"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_profile_validate_warnings_still_ok_true() {
    let home = temp_home("validate_warnings");
    write_profile(&home, "mode: rule\n");
    let output = run_with_home(&home, &["--json", "profile", "validate"]);
    assert!(
        output.status.success(),
        "validate 有 warnings 时仍应退出 0: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "profile.validate");
    assert!(
        value["warnings"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "应返回 warnings 数组: {value}"
    );
    assert!(value.get("error").is_none() || value["error"].is_null());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn profile_update_help_describes_fetch_and_apply() {
    let output = std::process::Command::new(common::binary_path())
        .args(["profile", "update", "--help"])
        .output()
        .expect("help 失败");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fetch") || stdout.contains("拉取"));
    assert!(stdout.contains("渲染") || stdout.contains("生效"));
}

#[test]
fn profile_update_offline_file_url_renders() {
    let home = temp_home("profile_update");
    let origin = home.join("origin.yaml");
    fs::write(
        &origin,
        "proxies: []\nrules:\n  - MATCH,DIRECT\nmode: rule\n",
    )
    .expect("写 origin 失败");
    let url = format!("file://{}", origin.display());
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    let index = serde_json::json!({
        "active": "main",
        "profiles": [{
            "name": "main",
            "url": url,
            "file": "main.yaml",
            "created_at": 1,
            "updated_at": 1
        }]
    });
    fs::write(
        profiles.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    fs::write(profiles.join("main.yaml"), "proxies: []\n").unwrap();

    let output = run_with_home(
        &home,
        &["profile", "update", "--name", "main", "--no-restart"],
    );
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "profile.update");
    assert_eq!(value["fetched"], true);
    assert_eq!(value["applied"], true);
    assert_eq!(value["restarted"], false);
    let runtime = read_runtime_yaml(&home);
    assert!(runtime.get("rules").is_some());
    assert_eq!(runtime.get("mode").and_then(|v| v.as_str()), Some("rule"));
    let via_sub = run_with_home(
        &home,
        &["--json", "sub", "update", "--name", "main", "--no-restart"],
    );
    let sub_json = assert_json_ok(&via_sub);
    assert_eq!(sub_json["action"], "profile.update");
    assert_eq!(sub_json["applied"], true);
    assert_eq!(sub_json["restarted"], false);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn profile_update_rejects_bad_upstream_without_overwriting_good_files() {
    let home = temp_home("profile_bad_upstream");
    let origin = home.join("origin.yaml");
    let good = "proxies: []\nrules:\n  - MATCH,DIRECT\nmode: rule\n";
    fs::write(&origin, good).unwrap();
    let url = format!("file://{}", origin.display());
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    let index = serde_json::json!({
        "active": "main",
        "profiles": [{"name":"main","url":url,"file":"main.yaml","created_at":1,"updated_at":1}]
    });
    fs::write(
        profiles.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    fs::write(profiles.join("main.yaml"), good).unwrap();
    assert_json_ok(&run_with_home(&home, &["--json", "profile", "render"]));
    let runtime_before = fs::read(home.join("runtime/config.yaml")).unwrap();
    let profile_before = fs::read(profiles.join("main.yaml")).unwrap();

    fs::write(&origin, "upstream temporarily unavailable\n").unwrap();
    assert_json_err(&run_with_home(
        &home,
        &["--json", "profile", "update", "--no-restart"],
    ));
    assert_eq!(
        fs::read(profiles.join("main.yaml")).unwrap(),
        profile_before
    );
    assert_eq!(
        fs::read(home.join("runtime/config.yaml")).unwrap(),
        runtime_before
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn failed_profile_use_apply_keeps_previous_active() {
    let home = temp_home("failed_use_keeps_active");
    write_profile(&home, &fixture_text("subscription-no-tun.yaml"));
    let profiles = home.join("profiles");
    let mut index: serde_json::Value =
        serde_json::from_slice(&fs::read(profiles.join("index.json")).unwrap()).unwrap();
    index["profiles"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "missing",
            "url": "file:///definitely/missing/clash.yaml",
            "file": "missing.yaml",
            "created_at": 2,
            "updated_at": null
        }));
    fs::write(
        profiles.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();

    assert_json_err(&run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "use",
            "--name",
            "missing",
            "--apply",
            "--no-restart",
        ],
    ));
    let listed = assert_json_ok(&run_with_home(&home, &["--json", "profile", "list"]));
    assert_eq!(listed["active"], "main");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn named_profile_update_also_commits_active_profile() {
    let home = temp_home("named_update_active");
    let profiles = home.join("profiles");
    fs::create_dir_all(&profiles).unwrap();
    let first_origin = home.join("first.yaml");
    let second_origin = home.join("second.yaml");
    fs::write(
        &first_origin,
        "proxies: []\nrules: [MATCH,DIRECT]\nmode: rule\n",
    )
    .unwrap();
    fs::write(
        &second_origin,
        "proxies: []\nrules: [MATCH,REJECT]\nmode: global\n",
    )
    .unwrap();
    let index = serde_json::json!({
        "active": "first",
        "profiles": [
            {"name":"first","url":format!("file://{}", first_origin.display()),"file":"first.yaml","created_at":1,"updated_at":1},
            {"name":"second","url":format!("file://{}", second_origin.display()),"file":"second.yaml","created_at":2,"updated_at":1}
        ]
    });
    fs::write(
        profiles.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    fs::write(
        profiles.join("first.yaml"),
        fs::read(&first_origin).unwrap(),
    )
    .unwrap();
    fs::write(
        profiles.join("second.yaml"),
        fs::read(&second_origin).unwrap(),
    )
    .unwrap();

    assert_json_ok(&run_with_home(
        &home,
        &[
            "--json",
            "profile",
            "update",
            "--name",
            "second",
            "--no-restart",
        ],
    ));
    let listed = assert_json_ok(&run_with_home(&home, &["--json", "profile", "list"]));
    assert_eq!(listed["active"], "second");
    let runtime = read_runtime_yaml(&home);
    assert_eq!(runtime.get("mode").and_then(|v| v.as_str()), Some("global"));
    let _ = fs::remove_dir_all(&home);
}

fn assert_node_command_is_not_env_state(output: &std::process::Output) {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("未找到代理状态"),
        "节点命令不得走 proxy.state: {combined}"
    );
    assert!(
        !combined.contains("请先执行 `clash proxy start`")
            && !combined.contains("请先执行 clash proxy start"),
        "节点命令不得要求 proxy start: {combined}"
    );
}

#[test]
fn proxy_list_is_api_not_env_state() {
    let home = temp_home("proxy_list_api");
    let output = run_with_home(
        &home,
        &[
            "--json",
            "proxy",
            "list",
            "--controller",
            "127.0.0.1:1",
            "--timeout-secs",
            "1",
        ],
    );
    assert_node_command_is_not_env_state(&output);
    if !output.status.success() {
        let value = assert_json_err(&output);
        let err = value["error"].as_str().unwrap_or("");
        assert!(
            err.contains("controller")
                || err.contains("连接")
                || err.contains("请求")
                || err.contains("配置")
                || err.contains("失败"),
            "失败应是 API/controller，不是 env 状态: {err}"
        );
    }
    let alias = run_with_home(
        &home,
        &[
            "--json",
            "api",
            "proxies",
            "--controller",
            "127.0.0.1:1",
            "--timeout-secs",
            "1",
        ],
    );
    assert_node_command_is_not_env_state(&alias);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn proxy_switch_is_api_not_env_state() {
    let home = temp_home("proxy_switch_api");
    let output = run_with_home(
        &home,
        &[
            "--json",
            "proxy",
            "switch",
            "--group",
            "Proxy",
            "--proxy",
            "direct",
            "--controller",
            "127.0.0.1:1",
            "--timeout-secs",
            "1",
        ],
    );
    assert_node_command_is_not_env_state(&output);
    if !output.status.success() {
        let value = assert_json_err(&output);
        let err = value["error"].as_str().unwrap_or("");
        assert!(
            !err.contains("未找到代理状态"),
            "switch 失败不得来自 env writer: {err}"
        );
    }
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn system_status_is_desktop_not_http() {
    let home = temp_home("system_status_desktop");
    let output = run_with_home(&home, &["--json", "system", "status"]);
    assert!(
        output.status.success(),
        "system status 应成功: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "proxy.system.status");
    assert!(value.get("enabled").is_some());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("external-controller") && !combined.contains("/proxies"),
        "system 不得走 mihomo HTTP: {combined}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn proxy_system_status_reports_backend_or_hint() {
    let home = temp_home("sys_proxy");
    let output = run_with_home(&home, &["proxy", "system", "status"]);
    assert!(output.status.success());
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "proxy.system.status");
    assert!(value.get("enabled").is_some());
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn piped_ui_status_is_json_without_flag() {
    let home = temp_home("ui_status");
    let output = run_with_home(&home, &["ui", "status"]);
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "ui.status");
    assert_eq!(value["installed"], false);
    assert_eq!(value["name"], "metacubexd");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn piped_profile_list_is_json_without_json_flag() {
    let home = temp_home("piped_list");
    let output = run_with_home(&home, &["profile", "list"]);
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "profile.list");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn text_flag_prints_human_profile_list() {
    let home = temp_home("text_list");
    let output = run_with_home(&home, &["--text", "profile", "list"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        " --text 不应输出 JSON: {stdout}"
    );
    assert!(
        stdout.contains("暂无 profile") || stdout.contains("当前配置目录"),
        "人读输出应说明当前状态: {stdout}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn piped_proxy_env_stays_shell_script() {
    let home = temp_home("piped_env");
    let output = run_with_home(&home, &["proxy", "env", "off"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("unset http_proxy"));
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        "proxy env 管道里仍应是 shell: {stdout}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn piped_env_on_stays_shell_script() {
    let home = temp_home("piped_env_top");
    let start = run_with_home(
        &home,
        &[
            "proxy",
            "start",
            "--host",
            "127.0.0.1",
            "--http-port",
            "17890",
            "--socks-port",
            "17891",
        ],
    );
    assert!(
        start.status.success(),
        "proxy start 别名应成功: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let output = run_with_home(&home, &["env", "on"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("export http_proxy") || stdout.contains("http_proxy="),
        "env on 应为 shell 导出脚本: {stdout}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err(),
        "env on 管道里仍应是 shell: {stdout}"
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn sub_list_json_matches_profile_list() {
    let home = temp_home("sub_list");
    let profile = run_with_home(&home, &["--json", "profile", "list"]);
    let sub = run_with_home(&home, &["--json", "sub", "list"]);
    let profile_json = assert_json_ok(&profile);
    let sub_json = assert_json_ok(&sub);
    assert_eq!(profile_json["action"], "profile.list");
    assert_eq!(sub_json["action"], profile_json["action"]);
    assert_eq!(sub_json["profiles"], profile_json["profiles"]);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn verge_help_roles_and_aliases() {
    let bin = common::binary_path();
    let help = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .expect("help 失败");
    assert!(help.status.success());
    let top = String::from_utf8_lossy(&help.stdout);
    for cmd in ["sub", "system", "mode", "env", "ui", "tun", "proxy"] {
        assert!(top.contains(cmd), "顶层帮助缺少 {cmd}: {top}");
    }
    assert!(
        top.contains("代理组") || top.contains("节点"),
        "proxy 应为选节点/代理组: {top}"
    );
    assert!(
        !top.contains("管理终端代理"),
        "proxy 不应再写终端代理: {top}"
    );

    let proxy_help = std::process::Command::new(bin)
        .args(["proxy", "--help"])
        .output()
        .expect("proxy help 失败");
    let proxy_txt = String::from_utf8_lossy(&proxy_help.stdout);
    assert!(
        proxy_txt.contains("代理组") || proxy_txt.contains("节点"),
        "proxy --help 应讲代理组/节点: {proxy_txt}"
    );
    assert!(
        !proxy_txt.contains("终端代理环境变量"),
        "proxy --help 不应是终端环境变量: {proxy_txt}"
    );
    assert!(
        proxy_txt.contains("list") && proxy_txt.contains("switch"),
        "proxy --help 应列出 list/switch: {proxy_txt}"
    );

    let system_help = std::process::Command::new(bin)
        .args(["system", "--help"])
        .output()
        .expect("system help 失败");
    let system_txt = String::from_utf8_lossy(&system_help.stdout);
    assert!(
        system_txt.contains("系统代理"),
        "system --help 应描述系统代理: {system_txt}"
    );

    let env_help = std::process::Command::new(bin)
        .args(["env", "--help"])
        .output()
        .expect("env help 失败");
    let env_txt = String::from_utf8_lossy(&env_help.stdout);
    assert!(
        env_txt.contains("环境变量") || env_txt.contains("eval"),
        "env --help 应描述复制环境变量: {env_txt}"
    );

    let sub_update = std::process::Command::new(bin)
        .args(["sub", "update", "--help"])
        .output()
        .expect("sub update help 失败");
    let sub_upd = String::from_utf8_lossy(&sub_update.stdout);
    assert!(sub_upd.contains("fetch") || sub_upd.contains("拉取"));
    assert!(sub_upd.contains("渲染") || sub_upd.contains("生效"));

    let home = temp_home("alias_ok");
    let profile_list = run_with_home(&home, &["--json", "profile", "list"]);
    assert_json_ok(&profile_list);
    let env_alias = run_with_home(&home, &["proxy", "env", "off"]);
    assert!(env_alias.status.success());
    let sys_alias = run_with_home(&home, &["proxy", "system", "status"]);
    assert!(
        sys_alias.status.success(),
        "proxy system status 别名应成功: stdout={} stderr={}",
        String::from_utf8_lossy(&sys_alias.stdout),
        String::from_utf8_lossy(&sys_alias.stderr)
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn json_env_enables_json_without_flag() {
    let home = temp_home("json_env");
    let output = std::process::Command::new(common::binary_path())
        .args(["profile", "list"])
        .env("CLASH_CLI_HOME", &home)
        .env("CLASH_CLI_JSON", "true")
        .env("CLASH_CLI_NO_AUTO_SUDO", "1")
        .output()
        .expect("执行失败");
    let value = assert_json_ok(&output);
    assert_eq!(value["action"], "profile.list");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn core_version_and_path_uninstalled_json() {
    let home = temp_home("core_uninstalled");
    let version = run_with_home(&home, &["--json", "core", "version"]);
    let version_json = assert_json_ok(&version);
    assert_eq!(version_json["installed"], false);

    let path = run_with_home(&home, &["--json", "core", "path"]);
    let path_json = assert_json_ok(&path);
    assert_eq!(path_json["installed"], false);
    let _ = fs::remove_dir_all(&home);
}
