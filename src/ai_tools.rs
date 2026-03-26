use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::ai_protocol::ToolDef;
use crate::paths::app_paths;

/// mihomo API 上下文
pub struct MihomoCtx {
    pub base_url: String,
    pub secret: Option<String>,
    pub client: reqwest::blocking::Client,
}

impl MihomoCtx {
    pub fn new(controller: &str, secret: Option<String>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("创建 mihomo API 客户端失败")?;
        let base_url = normalize_url(controller);
        Ok(Self {
            base_url,
            secret,
            client,
        })
    }
}

/// 判断工具是否为写操作（会修改配置或重载服务）
pub fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "set_mixin_field" | "unset_mixin_field" | "render_profile" | "reload_config"
    )
}

/// 执行工具调用，返回 JSON 字符串结果
pub fn execute_tool(name: &str, arguments: &str, mihomo: &MihomoCtx, dry_run: bool) -> String {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Object(Default::default()));
    let result = match name {
        "get_connections" => tool_get_connections(mihomo, &args),
        "get_proxies" => tool_get_proxies(mihomo),
        "get_rules" => tool_get_rules(mihomo),
        "get_mixin" => tool_get_mixin(),
        "set_mixin_field" => {
            if dry_run {
                Ok(serde_json::json!({"ok": true, "dry_run": true, "message": "模拟执行成功"}))
            } else {
                tool_set_mixin_field(&args)
            }
        }
        "unset_mixin_field" => {
            if dry_run {
                Ok(serde_json::json!({"ok": true, "dry_run": true, "message": "模拟执行成功"}))
            } else {
                tool_unset_mixin_field(&args)
            }
        }
        "render_profile" => {
            if dry_run {
                Ok(serde_json::json!({"ok": true, "dry_run": true, "message": "模拟执行成功"}))
            } else {
                tool_render_profile()
            }
        }
        "reload_config" => {
            if dry_run {
                Ok(serde_json::json!({"ok": true, "dry_run": true, "message": "模拟执行成功"}))
            } else {
                tool_reload_config(mihomo)
            }
        }
        _ => Ok(serde_json::json!({"error": format!("未知工具: {name}")})),
    };

    match result {
        Ok(v) => v.to_string(),
        Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
    }
}

fn tool_get_connections(ctx: &MihomoCtx, args: &Value) -> Result<Value> {
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
    let data = mihomo_get(&ctx.client, &ctx.base_url, "/connections", &ctx.secret)?;

    let connections = data.get("connections").and_then(|v| v.as_array());
    let summary: Vec<Value> = connections
        .map(|arr| {
            arr.iter()
                .take(limit)
                .map(|conn| {
                    let meta = conn.get("metadata").unwrap_or(conn);
                    serde_json::json!({
                        "host": meta.get("host").and_then(|v| v.as_str()).unwrap_or(""),
                        "destination": meta.get("destinationIP").and_then(|v| v.as_str()).unwrap_or(""),
                        "network": meta.get("network").and_then(|v| v.as_str()).unwrap_or(""),
                        "type": meta.get("type").and_then(|v| v.as_str()).unwrap_or(""),
                        "chains": conn.get("chains"),
                        "rule": conn.get("rule").and_then(|v| v.as_str()).unwrap_or(""),
                        "rulePayload": conn.get("rulePayload").and_then(|v| v.as_str()).unwrap_or(""),
                        "download": conn.get("download").and_then(|v| v.as_u64()).unwrap_or(0),
                        "upload": conn.get("upload").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(serde_json::json!({
        "total": connections.map(|a| a.len()).unwrap_or(0),
        "returned": summary.len(),
        "connections": summary,
    }))
}

fn tool_get_proxies(ctx: &MihomoCtx) -> Result<Value> {
    mihomo_get(&ctx.client, &ctx.base_url, "/proxies", &ctx.secret)
}

fn tool_get_rules(ctx: &MihomoCtx) -> Result<Value> {
    mihomo_get(&ctx.client, &ctx.base_url, "/rules", &ctx.secret)
}

fn tool_get_mixin() -> Result<Value> {
    let paths = app_paths()?;
    if !paths.profile_mixin_file.exists() {
        return Ok(serde_json::json!({"exists": false, "content": null}));
    }
    let content =
        std::fs::read_to_string(&paths.profile_mixin_file).context("读取 mixin.yaml 失败")?;
    Ok(serde_json::json!({"exists": true, "content": content}))
}

fn tool_set_mixin_field(args: &Value) -> Result<Value> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .context("缺少 key 参数")?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .context("缺少 value 参数")?;

    // 复用 mixin 模块的 set 逻辑
    let mixin_args = crate::cli::MixinSetArgs {
        key: key.to_string(),
        value: value.to_string(),
    };
    crate::mixin::run(crate::cli::MixinCommand::Set(mixin_args))?;

    Ok(serde_json::json!({"ok": true, "key": key, "value": value}))
}

fn tool_unset_mixin_field(args: &Value) -> Result<Value> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .context("缺少 key 参数")?;

    let mixin_args = crate::cli::MixinSetArgs {
        key: key.to_string(),
        value: String::new(),
    };
    crate::mixin::run(crate::cli::MixinCommand::Unset(mixin_args))?;

    Ok(serde_json::json!({"ok": true, "key": key}))
}

fn tool_render_profile() -> Result<Value> {
    let args = crate::cli::ProfileRenderArgs {
        name: None,
        output: None,
        no_mixin: false,
        follow_subscription_port: false,
    };
    crate::profile::run(crate::cli::ProfileCommand::Render(args))?;
    Ok(serde_json::json!({"ok": true, "message": "profile render 完成"}))
}

fn tool_reload_config(ctx: &MihomoCtx) -> Result<Value> {
    let paths = app_paths()?;
    let config_path = paths.runtime_config_file.display().to_string();
    let payload = serde_json::json!({"path": config_path});

    let url = format!("{}/configs?force=true", ctx.base_url);
    let mut req = ctx.client.put(&url).json(&payload);
    if let Some(secret) = &ctx.secret {
        if !secret.is_empty() {
            req = req.header("Authorization", format!("Bearer {secret}"));
        }
    }
    let resp = req.send().context("通知 mihomo 重新加载失败")?;

    if resp.status().is_success() {
        Ok(serde_json::json!({"ok": true, "message": "mihomo 已重新加载配置"}))
    } else {
        let text = resp.text().unwrap_or_default();
        Ok(serde_json::json!({"ok": false, "error": text}))
    }
}

// --- helpers ---

fn mihomo_get(
    client: &reqwest::blocking::Client,
    base_url: &str,
    path: &str,
    secret: &Option<String>,
) -> Result<Value> {
    let url = format!("{base_url}{path}");
    let mut req = client.get(&url);
    if let Some(s) = secret {
        if !s.is_empty() {
            req = req.header("Authorization", format!("Bearer {s}"));
        }
    }
    let resp = req
        .send()
        .with_context(|| format!("请求 mihomo API 失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("mihomo API 返回错误: {url}"))?;
    resp.json::<Value>()
        .with_context(|| format!("解析 mihomo 响应失败: {url}"))
}

fn normalize_url(s: &str) -> String {
    if s.starts_with("http://") || s.starts_with("https://") {
        s.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", s.trim_end_matches('/'))
    }
}

/// 返回所有可用工具的 JSON Schema 定义
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "get_connections".into(),
            description: "获取 mihomo 当前活跃连接列表，包含域名、规则、代理链路等信息".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "最多返回连接数，默认 200"
                    }
                }
            }),
        },
        ToolDef {
            name: "get_proxies".into(),
            description: "获取所有代理组和节点信息，包含代理名称、类型、延迟等".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "get_rules".into(),
            description: "获取当前生效的路由规则列表".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "get_mixin".into(),
            description: "读取当前 mixin.yaml 覆盖配置的内容".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "set_mixin_field".into(),
            description: "设置 mixin.yaml 中的字段（YAML 点分路径），会在 render 后生效".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "YAML 点分路径，如 rules 或 dns.enable"},
                    "value": {"type": "string", "description": "要设置的值，自动推导类型"}
                },
                "required": ["key", "value"]
            }),
        },
        ToolDef {
            name: "unset_mixin_field".into(),
            description: "删除 mixin.yaml 中的指定字段".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "YAML 点分路径"}
                },
                "required": ["key"]
            }),
        },
        ToolDef {
            name: "render_profile".into(),
            description: "执行 profile render，将 mixin 合并到运行配置中".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "reload_config".into(),
            description: "通知 mihomo 内核重新加载配置文件，使规则变更即时生效".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
    ]
}
