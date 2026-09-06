use std::fmt::Write as _;
use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::cli::{
    ApiCommand, ApiCommonArgs, ApiConfigPatchArgs, ApiLogsArgs, ApiModeCommand, ApiProxySwitchArgs,
};
use crate::constants;
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

#[derive(Debug, Clone)]
struct ApiContext {
    base_url: String,
    secret: Option<String>,
}

pub fn run(command: ApiCommand) -> Result<()> {
    match command {
        ApiCommand::Status(common) => cmd_status(common),
        ApiCommand::Mode { action, common } => cmd_mode(action, common),
        ApiCommand::Proxies(common) => cmd_proxies(common),
        ApiCommand::Connections(common) => cmd_connections(common),
        ApiCommand::UiUrl(common) => cmd_ui_url(common),
        ApiCommand::Rules(common) => cmd_rules(common),
        ApiCommand::Configs(common) => cmd_configs(common),
        ApiCommand::Providers(common) => cmd_providers(common),
        ApiCommand::ProxySwitch(args) => cmd_proxy_switch(args),
        ApiCommand::CloseConnections(common) => cmd_close_connections(common),
        ApiCommand::ConfigPatch(args) => cmd_config_patch(args),
        ApiCommand::Traffic(common) => cmd_traffic(common),
        ApiCommand::Logs(args) => cmd_logs(args),
    }
}

fn cmd_status(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;

    let version = api_get(&client, &ctx, "/version");
    let response = match version {
        Ok(v) => v,
        Err(_) => api_get(&client, &ctx, "/configs")?,
    };

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.status",
            "controller": ctx.base_url,
            "response": response
        }));
    }

    println!("控制器地址: {}", ctx.base_url);
    if let Some(version) = response.get("version").and_then(|v| v.as_str()) {
        println!("内核版本: {}", version);
    }
    if let Some(mode) = response.get("mode").and_then(|v| v.as_str()) {
        println!("当前模式: {}", mode);
    }
    Ok(())
}

fn cmd_mode(action: ApiModeCommand, common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;

    match action {
        ApiModeCommand::Get => {
            let response = api_get(&client, &ctx, "/configs")?;
            let mode = response
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "api.mode.get",
                    "mode": mode,
                    "response": response
                }));
            }

            println!("当前模式: {}", mode);
        }
        ApiModeCommand::Set(args) => {
            let target = args.mode.as_api_str();
            let payload = serde_json::json!({ "mode": target });
            let response = api_patch(&client, &ctx, "/configs", payload)?;

            if is_json_mode() {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "api.mode.set",
                    "mode": target,
                    "response": response
                }));
            }

            println!("模式已设置为: {}", target);
        }
    }
    Ok(())
}

fn cmd_proxies(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_get(&client, &ctx, "/proxies")?;
    let groups = summarize_proxy_groups(&response);

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.proxies",
            "groups": groups_to_json(&groups),
            "response": response
        }));
    }

    print!("{}", format_proxy_groups_text(&groups));
    Ok(())
}

fn cmd_connections(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_get(&client, &ctx, "/connections")?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.connections",
            "response": response
        }));
    }

    print!("{}", format_connections_text(&response));
    Ok(())
}

fn cmd_ui_url(common: ApiCommonArgs) -> Result<()> {
    let ctx = load_api_context(&common)?;
    let paths = app_paths()?;
    let ui = load_runtime_ui_fields(&paths.runtime_config_file)?;
    let dashboard = build_dashboard_url(&ctx.base_url);

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.ui-url",
            "controller": ctx.base_url,
            "dashboard_url": dashboard,
            "external_ui": ui.external_ui,
            "external_ui_name": ui.external_ui_name,
            "external_ui_url": ui.external_ui_url,
            "secret_set": ctx.secret.as_ref().map(|v| !v.is_empty()).unwrap_or(false)
        }));
    }

    println!("控制器地址: {}", ctx.base_url);
    println!("Dashboard 地址: {}", dashboard);
    if let Some(v) = ui.external_ui {
        println!("external-ui: {}", v);
    }
    if let Some(v) = ui.external_ui_name {
        println!("external-ui-name: {}", v);
    }
    if let Some(v) = ui.external_ui_url {
        println!("external-ui-url: {}", v);
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct RuntimeUiFields {
    external_ui: Option<String>,
    external_ui_name: Option<String>,
    external_ui_url: Option<String>,
}

fn build_client(timeout_secs: u64) -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(8))
        .build()
        .context("创建 API 客户端失败")
}

fn load_api_context(common: &ApiCommonArgs) -> Result<ApiContext> {
    let paths = app_paths()?;

    let (config_controller, config_secret) = load_runtime_api_fields(&paths.runtime_config_file)?;
    let controller = common
        .controller
        .clone()
        .or(config_controller)
        .unwrap_or_else(|| constants::DEFAULT_CONTROLLER.to_string());
    let secret = common.secret.clone().or(config_secret);

    Ok(ApiContext {
        base_url: normalize_controller_url(&controller),
        secret,
    })
}

fn load_runtime_api_fields(path: &std::path::Path) -> Result<(Option<String>, Option<String>)> {
    if !path.exists() {
        return Ok((None, None));
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?;
    let root: YamlValue = serde_yaml::from_str(&content).context("解析 runtime 配置失败")?;
    let controller = yaml_key_string(&root, "external-controller");
    let secret = yaml_key_string(&root, "secret");
    Ok((controller, secret))
}

fn load_runtime_ui_fields(path: &std::path::Path) -> Result<RuntimeUiFields> {
    if !path.exists() {
        return Ok(RuntimeUiFields::default());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?;
    let root: YamlValue = serde_yaml::from_str(&content).context("解析 runtime 配置失败")?;
    Ok(RuntimeUiFields {
        external_ui: yaml_key_string(&root, "external-ui"),
        external_ui_name: yaml_key_string(&root, "external-ui-name"),
        external_ui_url: yaml_key_string(&root, "external-ui-url"),
    })
}

fn yaml_key_string(root: &YamlValue, key: &str) -> Option<String> {
    root.as_mapping()
        .and_then(|m| m.get(YamlValue::String(key.to_string())))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn normalize_controller_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", value.trim_end_matches('/'))
    }
}

fn build_dashboard_url(controller: &str) -> String {
    format!("{}/ui", controller.trim_end_matches('/'))
}

fn apply_secret(req: RequestBuilder, ctx: &ApiContext) -> RequestBuilder {
    if let Some(secret) = &ctx.secret
        && !secret.is_empty()
    {
        return req.header("Authorization", format!("Bearer {}", secret));
    }
    req
}

fn send_and_parse(req: RequestBuilder, url: &str) -> Result<JsonValue> {
    let resp = req
        .send()
        .with_context(|| format!("请求失败: {}", url))?
        .error_for_status()
        .with_context(|| format!("请求返回非成功状态: {}", url))?;
    parse_response(resp, url)
}

fn parse_response(resp: Response, url: &str) -> Result<JsonValue> {
    let text = resp
        .text()
        .with_context(|| format!("读取响应失败: {}", url))?;
    if text.is_empty() {
        Ok(serde_json::json!(null))
    } else {
        serde_json::from_str(&text).with_context(|| format!("解析响应失败: {}", url))
    }
}

fn api_get(client: &Client, ctx: &ApiContext, path: &str) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    send_and_parse(apply_secret(client.get(&url), ctx), &url)
}

fn api_patch(
    client: &Client,
    ctx: &ApiContext,
    path: &str,
    payload: JsonValue,
) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    send_and_parse(apply_secret(client.patch(&url).json(&payload), ctx), &url)
}

fn api_put(client: &Client, ctx: &ApiContext, path: &str, payload: JsonValue) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    send_and_parse(apply_secret(client.put(&url).json(&payload), ctx), &url)
}

fn api_delete(client: &Client, ctx: &ApiContext, path: &str) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    send_and_parse(apply_secret(client.delete(&url), ctx), &url)
}

fn cmd_rules(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_get(&client, &ctx, "/rules")?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.rules",
            "response": response
        }));
    }

    let empty = vec![];
    let rules = response
        .get("rules")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    println!("规则总数: {}", rules.len());
    let mut type_counts = std::collections::HashMap::new();
    for rule in rules {
        if let Some(t) = rule.get("type").and_then(|v| v.as_str()) {
            *type_counts.entry(t.to_string()).or_insert(0u64) += 1;
        }
    }
    for (t, count) in &type_counts {
        println!("  {}: {}", t, count);
    }
    Ok(())
}

fn cmd_configs(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_get(&client, &ctx, "/configs")?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.configs",
            "response": response
        }));
    }

    for key in &[
        "mode",
        "port",
        "socks-port",
        "mixed-port",
        "redir-port",
        "tproxy-port",
        "allow-lan",
        "log-level",
        "ipv6",
    ] {
        if let Some(v) = response.get(key) {
            println!("{}: {}", key, v);
        }
    }
    Ok(())
}

fn cmd_providers(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_get(&client, &ctx, "/providers/proxies")?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.providers",
            "response": response
        }));
    }

    let empty = serde_json::Map::new();
    let providers = response
        .get("providers")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    println!("Provider 数量: {}", providers.len());
    for (name, info) in providers {
        let ptype = info
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let count = info
            .get("proxies")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        println!("  {} (type={}, proxies={})", name, ptype, count);
    }
    Ok(())
}

fn cmd_proxy_switch(args: ApiProxySwitchArgs) -> Result<()> {
    let client = build_client(args.common.timeout_secs)?;
    let ctx = load_api_context(&args.common)?;
    let encoded_group = urlencoded(&args.group);
    let path = format!("/proxies/{}", encoded_group);
    let payload = serde_json::json!({ "name": args.proxy });
    let response = api_put(&client, &ctx, &path, payload)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.proxy-switch",
            "group": args.group,
            "proxy": args.proxy,
            "response": response
        }));
    }

    println!("已切换 [{}] -> {}", args.group, args.proxy);
    Ok(())
}

fn cmd_close_connections(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(common.timeout_secs)?;
    let ctx = load_api_context(&common)?;
    let response = api_delete(&client, &ctx, "/connections")?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.close-connections",
            "response": response
        }));
    }

    println!("已关闭所有连接。");
    Ok(())
}

fn cmd_config_patch(args: ApiConfigPatchArgs) -> Result<()> {
    let payload: JsonValue =
        serde_json::from_str(&args.data).context("--data 参数不是合法的 JSON")?;
    let client = build_client(args.common.timeout_secs)?;
    let ctx = load_api_context(&args.common)?;
    let response = api_patch(&client, &ctx, "/configs", payload)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.config-patch",
            "response": response
        }));
    }

    println!("配置已更新。");
    Ok(())
}

fn cmd_traffic(common: ApiCommonArgs) -> Result<()> {
    let client = build_client(3)?;
    let ctx = load_api_context(&common)?;
    let url = format!("{}/traffic", ctx.base_url);
    let req = apply_secret(client.get(&url), &ctx);
    let text = req
        .send()
        .with_context(|| format!("请求失败: {}", url))?
        .text()
        .unwrap_or_default();

    let snapshot = text
        .lines()
        .find_map(|line| serde_json::from_str::<JsonValue>(line).ok());

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.traffic",
            "snapshot": snapshot
        }));
    }

    print!("{}", format_traffic_text(snapshot.as_ref()));
    Ok(())
}

fn cmd_logs(args: ApiLogsArgs) -> Result<()> {
    let client = build_client(3)?;
    let ctx = load_api_context(&args.common)?;
    let url = match &args.level {
        Some(level) => format!("{}/logs?level={}", ctx.base_url, level.as_api_str()),
        None => format!("{}/logs", ctx.base_url),
    };
    let req = apply_secret(client.get(&url), &ctx);
    let text = req
        .send()
        .with_context(|| format!("请求失败: {}", url))?
        .text()
        .unwrap_or_default();

    let entries: Vec<JsonValue> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<JsonValue>(line).ok())
        .take(50)
        .collect();

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.logs",
            "count": entries.len(),
            "entries": entries
        }));
    }

    if entries.is_empty() {
        println!("未获取到日志条目。");
    } else {
        println!("获取到 {} 条日志:", entries.len());
        for entry in &entries {
            let level = entry
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let payload = entry.get("payload").and_then(|v| v.as_str()).unwrap_or("");
            println!("[{}] {}", level, payload);
        }
    }
    Ok(())
}

fn is_proxy_group_type(typ: &str) -> bool {
    matches!(
        typ,
        "Selector" | "URLTest" | "Fallback" | "LoadBalance" | "Relay" | "Smart"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyMemberView {
    name: String,
    delay_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyGroupView {
    name: String,
    type_name: String,
    now: Option<String>,
    members: Vec<ProxyMemberView>,
}

fn last_delay_ms(proxy: &JsonValue) -> Option<u64> {
    let history = proxy.get("history")?.as_array()?;
    let last = history.last()?;
    let delay = last.get("delay")?.as_u64()?;
    if delay == 0 { None } else { Some(delay) }
}

fn summarize_proxy_groups(response: &JsonValue) -> Vec<ProxyGroupView> {
    let Some(proxies) = response.get("proxies").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for (name, info) in proxies {
        let type_name = info
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if !is_proxy_group_type(type_name) {
            continue;
        }
        let now = info
            .get("now")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let all = info
            .get("all")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let members = all
            .iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .map(|member_name| {
                let delay_ms = proxies.get(&member_name).and_then(last_delay_ms);
                ProxyMemberView {
                    name: member_name,
                    delay_ms,
                }
            })
            .collect();
        groups.push(ProxyGroupView {
            name: name.clone(),
            type_name: type_name.to_string(),
            now,
            members,
        });
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

fn groups_to_json(groups: &[ProxyGroupView]) -> JsonValue {
    JsonValue::Array(
        groups
            .iter()
            .map(|g| {
                serde_json::json!({
                    "name": g.name,
                    "type": g.type_name,
                    "now": g.now,
                    "proxies": g.members.iter().map(|m| {
                        serde_json::json!({
                            "name": m.name,
                            "delay_ms": m.delay_ms,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn format_proxy_groups_text(groups: &[ProxyGroupView]) -> String {
    if groups.is_empty() {
        return "未发现代理组。内核未启动或订阅尚未生效。\n切节点: clash api proxy-switch --group <组> --proxy <节点>\n"
            .to_string();
    }
    let mut out = String::new();
    out.push_str("代理组（* 为当前节点）\n");
    out.push_str("切节点: clash api proxy-switch --group <组> --proxy <节点>\n");
    for group in groups {
        let now = group.now.as_deref().unwrap_or("-");
        let _ = writeln!(out, "\n[{}] {}  当前: {}", group.type_name, group.name, now);
        if group.members.is_empty() {
            out.push_str("  （无节点）\n");
            continue;
        }
        for member in &group.members {
            let mark = if Some(member.name.as_str()) == group.now.as_deref() {
                "*"
            } else {
                " "
            };
            match member.delay_ms {
                Some(ms) => {
                    let _ = writeln!(out, "  {mark} {}  {ms}ms", member.name);
                }
                None => {
                    let _ = writeln!(out, "  {mark} {}", member.name);
                }
            }
        }
    }
    out
}

fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let x = n as f64;
    if x < KB {
        format!("{n} B")
    } else if x < MB {
        format!("{:.1} KB", x / KB)
    } else if x < GB {
        format!("{:.1} MB", x / MB)
    } else {
        format!("{:.2} GB", x / GB)
    }
}

fn format_connections_text(response: &JsonValue) -> String {
    let empty = vec![];
    let list = response
        .get("connections")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let down = response
        .get("downloadTotal")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let up = response
        .get("uploadTotal")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut out = format!(
        "连接数: {}  累计 ↑{} ↓{}\n",
        list.len(),
        format_bytes(up),
        format_bytes(down)
    );
    for conn in list.iter().take(20) {
        let meta = conn.get("metadata");
        let host = meta
            .and_then(|m| m.get("host"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                meta.and_then(|m| m.get("destinationIP"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("-");
        let port = meta.and_then(|m| m.get("destinationPort")).and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        });
        let dest = match port {
            Some(p) if !p.is_empty() => format!("{host}:{p}"),
            _ => host.to_string(),
        };
        let chain = conn
            .get("chains")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let c_up = conn.get("upload").and_then(|v| v.as_u64()).unwrap_or(0);
        let c_down = conn.get("download").and_then(|v| v.as_u64()).unwrap_or(0);
        let _ = writeln!(
            out,
            "  {dest}  via {chain}  ↑{} ↓{}",
            format_bytes(c_up),
            format_bytes(c_down)
        );
    }
    if list.len() > 20 {
        let _ = writeln!(out, "  … 还有 {} 条", list.len() - 20);
    }
    out.push_str("关掉全部: clash api close-connections\n");
    out
}

fn format_traffic_text(snapshot: Option<&JsonValue>) -> String {
    match snapshot {
        Some(s) => {
            let up = s.get("up").and_then(|v| v.as_u64()).unwrap_or(0);
            let down = s.get("down").and_then(|v| v.as_u64()).unwrap_or(0);
            format!(
                "瞬时速率  ↑{}/s  ↓{}/s\n（快照，再执行一次即可刷新）\n",
                format_bytes(up),
                format_bytes(down)
            )
        }
        None => "未获取到流量。请确认服务已启动: clash service status\n".to_string(),
    }
}

fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_controller_url_should_add_http_when_scheme_missing() {
        assert_eq!(
            normalize_controller_url("127.0.0.1:9090"),
            "http://127.0.0.1:9090"
        );
        assert_eq!(
            normalize_controller_url("example.com:9999/"),
            "http://example.com:9999"
        );
    }

    #[test]
    fn normalize_controller_url_should_trim_trailing_slash() {
        assert_eq!(
            normalize_controller_url("http://127.0.0.1:9090/"),
            "http://127.0.0.1:9090"
        );
        assert_eq!(
            normalize_controller_url("https://a.b.c/controller/"),
            "https://a.b.c/controller"
        );
    }

    #[test]
    fn yaml_key_string_should_read_root_mapping_key() {
        let root: YamlValue = serde_yaml::from_str(
            r#"
external-controller: 127.0.0.1:9090
secret: abc
"#,
        )
        .expect("解析 YAML 失败");

        assert_eq!(
            yaml_key_string(&root, "external-controller").as_deref(),
            Some("127.0.0.1:9090")
        );
        assert_eq!(yaml_key_string(&root, "secret").as_deref(), Some("abc"));
        assert_eq!(yaml_key_string(&root, "missing"), None);
    }

    #[test]
    fn build_dashboard_url_should_append_ui_path() {
        assert_eq!(
            build_dashboard_url("http://127.0.0.1:9090"),
            "http://127.0.0.1:9090/ui"
        );
        assert_eq!(
            build_dashboard_url("http://127.0.0.1:9090/"),
            "http://127.0.0.1:9090/ui"
        );
    }

    #[test]
    fn summarize_proxy_groups_lists_selector_and_now() {
        let response = serde_json::json!({
            "proxies": {
                "香港": {
                    "type": "Shadowsocks",
                    "history": [{"delay": 80}]
                },
                "GLOBAL": {
                    "type": "Selector",
                    "now": "香港",
                    "all": ["香港", "DIRECT"]
                },
                "DIRECT": { "type": "Direct" }
            }
        });
        let groups = summarize_proxy_groups(&response);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "GLOBAL");
        assert_eq!(groups[0].now.as_deref(), Some("香港"));
        assert_eq!(groups[0].members[0].delay_ms, Some(80));
        let text = format_proxy_groups_text(&groups);
        assert!(text.contains("GLOBAL"));
        assert!(text.contains("* 香港"));
        assert!(text.contains("80ms"));
        assert!(text.contains("proxy-switch"));
    }

    #[test]
    fn format_traffic_and_connections_are_readable() {
        let traffic = format_traffic_text(Some(&serde_json::json!({"up": 2048, "down": 4096})));
        assert!(traffic.contains("↑") && traffic.contains("↓"));
        assert!(traffic.contains("KB"));
        let conns = format_connections_text(&serde_json::json!({
            "downloadTotal": 1024,
            "uploadTotal": 512,
            "connections": [{
                "upload": 10,
                "download": 20,
                "chains": ["香港"],
                "metadata": { "host": "example.com", "destinationPort": "443" }
            }]
        }));
        assert!(conns.contains("example.com:443"));
        assert!(conns.contains("via 香港"));
        assert!(conns.contains("连接数: 1"));
    }
}
