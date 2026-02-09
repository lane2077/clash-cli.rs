use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;

use crate::cli::{ApiCommand, ApiCommonArgs, ApiModeCommand};
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

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "api.proxies",
            "response": response
        }));
    }

    let count = response
        .get("proxies")
        .and_then(|v| v.as_object())
        .map(|m| m.len())
        .unwrap_or(0);
    println!("代理对象数量: {}", count);
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

    let total = response
        .get("connections")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let down = response
        .get("downloadTotal")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let up = response
        .get("uploadTotal")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    println!("连接数: {}", total);
    println!("总下行: {}", down);
    println!("总上行: {}", up);
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
        .unwrap_or_else(|| "127.0.0.1:9090".to_string());
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
    if let Some(secret) = &ctx.secret {
        if !secret.is_empty() {
            return req.header("Authorization", format!("Bearer {}", secret));
        }
    }
    req
}

fn api_get(client: &Client, ctx: &ApiContext, path: &str) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    let req = apply_secret(client.get(&url), ctx);
    let resp = req
        .send()
        .with_context(|| format!("请求失败: {}", url))?
        .error_for_status()
        .with_context(|| format!("请求返回非成功状态: {}", url))?;
    resp.json::<JsonValue>()
        .with_context(|| format!("解析响应失败: {}", url))
}

fn api_patch(
    client: &Client,
    ctx: &ApiContext,
    path: &str,
    payload: JsonValue,
) -> Result<JsonValue> {
    let url = format!("{}{}", ctx.base_url, path);
    let req = apply_secret(client.patch(&url).json(&payload), ctx);
    let resp = req
        .send()
        .with_context(|| format!("请求失败: {}", url))?
        .error_for_status()
        .with_context(|| format!("请求返回非成功状态: {}", url))?;
    resp.json::<JsonValue>()
        .with_context(|| format!("解析响应失败: {}", url))
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
}
