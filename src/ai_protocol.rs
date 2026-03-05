use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// LLM 统一响应：文本回复或工具调用
pub enum LlmResponse {
    Text(String),
    ToolCalls(Vec<ToolCall>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub struct LlmConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub protocol: Protocol,
}

#[derive(Clone, Copy, Debug)]
pub enum Protocol {
    Completions,
    Responses,
}

pub fn build_llm_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("创建 LLM HTTP 客户端失败")
}

/// 获取可用模型列表（GET /v1/models）
pub fn list_models(client: &Client, api_base: &str, api_key: &str) -> Result<Vec<String>> {
    let url = format!("{}/models", api_base.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .with_context(|| format!("请求模型列表失败: {url}"))?;

    let status = response.status();
    let text = response.text().context("读取模型列表响应失败")?;

    if !status.is_success() {
        bail!("获取模型列表失败 ({}): {}", status, truncate(&text, 300));
    }

    let json: Value = serde_json::from_str(&text).context("解析模型列表 JSON 失败")?;
    let models = json
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let mut names: Vec<String> = arr
                .iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect();
            names.sort();
            names
        })
        .unwrap_or_default();

    Ok(models)
}

pub fn call_llm(
    client: &Client,
    config: &LlmConfig,
    messages: &[Value],
    tools: &[ToolDef],
) -> Result<LlmResponse> {
    match config.protocol {
        Protocol::Completions => call_completions(client, config, messages, tools),
        Protocol::Responses => call_responses(client, config, messages, tools),
    }
}

fn call_completions(
    client: &Client,
    config: &LlmConfig,
    messages: &[Value],
    tools: &[ToolDef],
) -> Result<LlmResponse> {
    let url = format!("{}/chat/completions", config.api_base.trim_end_matches('/'));

    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": messages,
    });

    if !tools_json.is_empty() {
        body["tools"] = Value::Array(tools_json);
    }

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .with_context(|| format!("请求 LLM 失败: {url}"))?;

    let status = response.status();
    let text = response.text().context("读取 LLM 响应失败")?;

    if !status.is_success() {
        bail!("LLM 返回错误 ({}): {}", status, truncate(&text, 500));
    }

    let json: Value = serde_json::from_str(&text).context("解析 LLM JSON 响应失败")?;

    let choice = json
        .get("choices")
        .and_then(|c| c.get(0))
        .context("LLM 响应中缺少 choices")?;

    let message = choice.get("message").context("choices[0] 缺少 message")?;

    // 优先检查 tool_calls
    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        let calls = parse_tool_calls_completions(tool_calls)?;
        if !calls.is_empty() {
            return Ok(LlmResponse::ToolCalls(calls));
        }
    }

    let content = message
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(LlmResponse::Text(content))
}

fn call_responses(
    client: &Client,
    config: &LlmConfig,
    messages: &[Value],
    tools: &[ToolDef],
) -> Result<LlmResponse> {
    let url = format!("{}/responses", config.api_base.trim_end_matches('/'));

    let tools_json: Vec<Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();

    let input = convert_messages_to_responses_input(messages);

    let mut body = serde_json::json!({
        "model": config.model,
        "input": input,
    });

    if !tools_json.is_empty() {
        body["tools"] = Value::Array(tools_json);
    }

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .with_context(|| format!("请求 Responses API 失败: {url}"))?;

    let status = response.status();
    let text = response.text().context("读取 Responses API 响应失败")?;

    if !status.is_success() {
        bail!("Responses API 错误 ({}): {}", status, truncate(&text, 500));
    }

    let json: Value = serde_json::from_str(&text).context("解析 Responses API JSON 失败")?;

    let output = json
        .get("output")
        .and_then(|v| v.as_array())
        .context("Responses API 缺少 output 数组")?;

    // 检查 function_call 类型
    let tool_calls: Vec<ToolCall> = output
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("function_call"))
        .filter_map(|item| {
            let id = item.get("call_id").and_then(|v| v.as_str())?.to_string();
            let name = item.get("name").and_then(|v| v.as_str())?.to_string();
            let arguments = item.get("arguments").and_then(|v| v.as_str())?.to_string();
            Some(ToolCall {
                id,
                name,
                arguments,
            })
        })
        .collect();

    if !tool_calls.is_empty() {
        return Ok(LlmResponse::ToolCalls(tool_calls));
    }

    // 取文本输出
    let text_content: String = output
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("message"))
        .filter_map(|item| {
            item.get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("text"))
                .and_then(|v| v.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(LlmResponse::Text(text_content))
}

fn parse_tool_calls_completions(raw: &[Value]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    for item in raw {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let function = item.get("function").context("tool_call 缺少 function")?;
        let name = function
            .get("name")
            .and_then(|v| v.as_str())
            .context("function 缺少 name")?
            .to_string();
        let arguments = function
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("{}")
            .to_string();
        calls.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    Ok(calls)
}

/// Chat Completions 的 messages 转换为 Responses API 的 input 格式
fn convert_messages_to_responses_input(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            match role {
                "system" => serde_json::json!({
                    "role": "developer",
                    "content": msg.get("content").cloned().unwrap_or(Value::Null),
                }),
                _ => msg.clone(),
            }
        })
        .collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// 构建 Completions 协议的工具调用结果消息
pub fn tool_result_message_completions(tool_call_id: &str, result: &str) -> Value {
    serde_json::json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": result,
    })
}

/// 构建 Completions 协议的 assistant 消息（含 tool_calls）
pub fn assistant_tool_calls_message(tool_calls_raw: &[ToolCall]) -> Value {
    let calls: Vec<Value> = tool_calls_raw
        .iter()
        .map(|tc| {
            serde_json::json!({
                "id": tc.id,
                "type": "function",
                "function": {
                    "name": tc.name,
                    "arguments": tc.arguments,
                }
            })
        })
        .collect();

    serde_json::json!({
        "role": "assistant",
        "tool_calls": calls,
    })
}

/// 构建 Responses 协议的工具调用结果消息
pub fn tool_result_message_responses(call_id: &str, result: &str) -> Value {
    serde_json::json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": result,
    })
}
