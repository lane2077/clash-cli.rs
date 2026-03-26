use std::io::IsTerminal;

use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;

use crate::ai_config;
use crate::ai_protocol::{
    self, LlmConfig, LlmResponse, Protocol, ToolDef, assistant_tool_calls_message,
    tool_result_message_completions, tool_result_message_responses,
};
use crate::ai_tools::{self, MihomoCtx};
use crate::cli::{AiCommand, AiModelsArgs, AiRulesArgs};
use crate::constants;
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

pub fn run(command: AiCommand) -> Result<()> {
    match command {
        AiCommand::Rules(args) => cmd_rules(args),
        AiCommand::Models(args) => cmd_models(args),
    }
}

fn cmd_models(args: AiModelsArgs) -> Result<()> {
    let mut cfg = ai_config::load()?;
    let api_key = args
        .api_key
        .clone()
        .or_else(|| cfg.api_key.clone())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .context("未提供 API Key。请设置 OPENAI_API_KEY 环境变量或使用 --api-key 参数")?;
    let api_base = args
        .api_base
        .clone()
        .or_else(|| cfg.api_base.clone())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

    cfg.api_key = Some(api_key.clone());
    cfg.api_base = Some(api_base.clone());
    let _ = ai_config::save(&cfg);

    let client = ai_protocol::build_llm_client()?;
    let models = ai_protocol::list_models(&client, &api_base, &api_key)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "ai.models",
            "count": models.len(),
            "models": models,
        }));
    }

    if models.is_empty() {
        println!("未获取到可用模型。");
        return Ok(());
    }

    println!("可用模型 ({} 个):", models.len());
    for (i, name) in models.iter().enumerate() {
        println!("  {:>3}. {}", i + 1, name);
    }
    Ok(())
}

fn cmd_rules(args: AiRulesArgs) -> Result<()> {
    let mut cfg = ai_config::load()?;
    let api_key = args
        .api_key
        .clone()
        .or_else(|| cfg.api_key.clone())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .context("未提供 API Key。请设置 OPENAI_API_KEY 环境变量或使用 --api-key 参数")?;

    let api_base = args
        .api_base
        .clone()
        .or_else(|| cfg.api_base.clone())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let init_model: Option<String> = args.model.clone().or_else(|| cfg.model.clone());
    let protocol_str = args
        .protocol
        .clone()
        .or_else(|| cfg.protocol.clone())
        .unwrap_or_else(|| "responses".to_string());

    let protocol = match protocol_str.as_str() {
        "responses" => Protocol::Responses,
        _ => Protocol::Completions,
    };

    let final_model = select_model(init_model.as_deref(), &api_base, &api_key)?;

    cfg.api_key = Some(api_key.clone());
    cfg.api_base = Some(api_base.clone());
    cfg.model = Some(final_model.clone());
    cfg.protocol = Some(protocol_str.clone());
    let _ = ai_config::save(&cfg);

    let config = LlmConfig {
        api_base: api_base.clone(),
        api_key,
        model: final_model.clone(),
        protocol,
    };

    let controller = resolve_controller(&args.controller)?;
    let mihomo = MihomoCtx::new(&controller, args.secret.clone())?;

    let client = ai_protocol::build_llm_client()?;
    let tools = ai_tools::tool_definitions();
    let system_prompt = build_system_prompt();

    let mut messages: Vec<Value> = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt,
    })];

    if !is_json_mode() {
        println!(
            "AI 规则分析启动（模型: {}，协议: {:?}）",
            final_model, protocol
        );
        if args.dry_run {
            println!("[dry-run 模式] 写操作将模拟执行，不会实际修改配置");
        }
        println!("输入消息与 AI 对话，输入 exit 或按 Ctrl+C 退出");
        println!("---");
    }

    // 初始用户消息
    let initial_msg = "请分析当前的连接情况和路由规则，找出配置不合理的地方，并给出优化建议。如果我没有使用 --dry-run 模式，你可以直接修改规则并验证效果。";
    messages.push(serde_json::json!({ "role": "user", "content": initial_msg }));

    // JSON 模式下只跑一次
    if is_json_mode() {
        return run_single_pass(&client, &config, &mut messages, &tools, &mihomo, &args);
    }

    // 多轮对话循环
    let is_interactive = std::io::stdin().is_terminal();
    loop {
        // Agent Loop: 处理当前消息
        let reply = run_agent_loop(&client, &config, &mut messages, &tools, &mihomo, &args);

        match reply {
            Ok(text) => println!("\n{}", text),
            Err(e) => eprintln!("\nAI 调用出错: {}", e),
        }

        if !is_interactive {
            return Ok(());
        }

        // 等待用户输入下一条消息
        println!();
        use std::io::Write;
        print!("You> ");
        std::io::stdout().flush().unwrap();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() || input.is_empty() {
            break;
        }
        let trimmed = input.trim();
        if trimmed.is_empty() || trimmed == "exit" || trimmed == "quit" {
            break;
        }

        messages.push(serde_json::json!({ "role": "user", "content": trimmed }));
    }

    println!("AI 对话结束。");
    Ok(())
}

fn run_single_pass(
    client: &Client,
    config: &LlmConfig,
    messages: &mut Vec<Value>,
    tools: &[ToolDef],
    mihomo: &MihomoCtx,
    args: &AiRulesArgs,
) -> Result<()> {
    let max_turns = args.max_turns;
    for turn in 0..max_turns {
        let response = ai_protocol::call_llm(client, config, messages, tools)
            .with_context(|| format!("第 {} 轮 LLM 调用失败", turn + 1))?;

        match response {
            LlmResponse::Text(text) => {
                return print_json(&serde_json::json!({
                    "ok": true,
                    "action": "ai.rules",
                    "turns": turn + 1,
                    "result": text,
                }));
            }
            LlmResponse::ToolCalls(tool_calls) => {
                // 将 assistant 的 tool_calls 消息加入历史
                match config.protocol {
                    Protocol::Completions => {
                        messages.push(assistant_tool_calls_message(&tool_calls));
                    }
                    Protocol::Responses => {
                        // Responses API 期望传入 function_call 原节点
                        for tc in &tool_calls {
                            messages.push(serde_json::json!({
                                "type": "function_call",
                                "id": format!("fc_{}", tc.id), // padding id
                                "call_id": tc.id,
                                "name": tc.name,
                                "arguments": tc.arguments,
                            }));
                        }
                    }
                }

                // 执行每个工具调用并将结果加入历史
                // JSON 模式下非 dry_run 时拒绝写操作（无法交互确认）
                for tc in &tool_calls {
                    let result = if ai_tools::is_write_tool(&tc.name) && !args.dry_run {
                        r#"{"error":"JSON 模式下不支持执行写操作，请使用 --dry-run 或交互模式"}"#
                            .to_string()
                    } else {
                        ai_tools::execute_tool(&tc.name, &tc.arguments, mihomo, args.dry_run)
                    };

                    let result_msg = match config.protocol {
                        Protocol::Completions => tool_result_message_completions(&tc.id, &result),
                        Protocol::Responses => tool_result_message_responses(&tc.id, &result),
                    };
                    messages.push(result_msg);
                }
            }
        }
    }

    print_json(&serde_json::json!({
        "ok": false,
        "action": "ai.rules",
        "error": format!("达到最大轮次限制 ({max_turns})"),
    }))
}

fn run_agent_loop(
    client: &Client,
    config: &LlmConfig,
    messages: &mut Vec<Value>,
    tools: &[ToolDef],
    mihomo: &MihomoCtx,
    args: &AiRulesArgs,
) -> Result<String> {
    let max_turns = args.max_turns;
    for turn in 0..max_turns {
        let response = ai_protocol::call_llm(client, config, messages, tools)
            .with_context(|| format!("第 {} 轮 LLM 调用失败", turn + 1))?;

        match response {
            LlmResponse::Text(text) => {
                return Ok(text);
            }
            LlmResponse::ToolCalls(tool_calls) => {
                if !is_json_mode() {
                    for tc in &tool_calls {
                        println!(
                            "[Turn {}] 调用工具: {}({})",
                            turn + 1,
                            tc.name,
                            truncate_args(&tc.arguments)
                        );
                    }
                }

                // 将 assistant 的 tool_calls 消息加入历史
                match config.protocol {
                    Protocol::Completions => {
                        messages.push(assistant_tool_calls_message(&tool_calls));
                    }
                    Protocol::Responses => {
                        // Responses API 期望传入 function_call 原节点
                        for tc in &tool_calls {
                            messages.push(serde_json::json!({
                                "type": "function_call",
                                "id": format!("fc_{}", tc.id), // padding id
                                "call_id": tc.id,
                                "name": tc.name,
                                "arguments": tc.arguments,
                            }));
                        }
                    }
                }

                // 执行每个工具调用并将结果加入历史
                for tc in &tool_calls {
                    let mut result = String::new();

                    if ai_tools::is_write_tool(&tc.name) && !args.dry_run {
                        if !ask_for_approval(&tc.name, &tc.arguments) {
                            result = "User denied permission to execute this tool.".to_string();
                        }
                    }

                    if result.is_empty() {
                        result =
                            ai_tools::execute_tool(&tc.name, &tc.arguments, mihomo, args.dry_run);
                    }

                    let result_msg = match config.protocol {
                        Protocol::Completions => tool_result_message_completions(&tc.id, &result),
                        Protocol::Responses => tool_result_message_responses(&tc.id, &result),
                    };
                    messages.push(result_msg);
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "达到最大轮次限制 ({})，AI 未能给出最终回复。",
        max_turns
    ))
}

fn build_system_prompt() -> String {
    r#"你是 mihomo/Clash 路由规则优化专家。你的任务是分析用户的网络连接日志和当前路由规则，找出配置不合理的地方，并给出优化建议。

你有以下工具可以使用：
- get_connections: 查看当前活跃连接（域名、使用的规则、代理链路等）
- get_proxies: 查看所有代理组和节点
- get_rules: 查看当前规则列表
- get_mixin: 查看 mixin.yaml（用户自定义覆盖规则）
- set_mixin_field: 修改 mixin.yaml 中的字段
- unset_mixin_field: 删除 mixin.yaml 中的字段
- render_profile: 重新渲染配置（使 mixin 变更生效）
- reload_config: 通知内核重新加载配置（使变更即时生效）

工作流程：
1. 先调用 get_connections 获取连接日志
2. 分析哪些域名的路由不合理（如国内域名走了代理、国外域名直连等）
3. 查看 get_rules 了解现有规则
4. 如果有问题，通过 set_mixin_field 添加修正规则
5. 调用 render_profile 和 reload_config 使变更生效
6. 再次 get_connections 验证效果

注意事项：
- 中国大陆常见域名（如 baidu.com, qq.com, taobao.com 等）应走 DIRECT
- 国际域名（如 google.com, github.com, openai.com 等）应走代理
- 优先使用 DOMAIN-SUFFIX 规则，覆盖范围更广
- mixin.yaml 中的 rules 字段是一个 YAML 列表，每项格式如 "DOMAIN-SUFFIX,google.com,Proxy"
- 输出最终分析报告时使用中文"#.to_string()
}

fn ask_for_approval(name: &str, args: &str) -> bool {
    if is_json_mode() || !std::io::stdin().is_terminal() {
        return false;
    }
    use std::io::Write;
    print!(
        "\n⚠️  AI 尝试执行写操作: {}({})\n是否允许执行? [y/N]: ",
        name, args
    );
    std::io::stdout().flush().unwrap();

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("y")
}

fn resolve_controller(explicit: &Option<String>) -> Result<String> {
    if let Some(c) = explicit {
        return Ok(c.clone());
    }
    // 尝试从 runtime config 读取
    let paths = app_paths()?;
    if paths.runtime_config_file.exists() {
        let content = std::fs::read_to_string(&paths.runtime_config_file).ok();
        if let Some(text) = content {
            let root: serde_yaml::Value = serde_yaml::from_str(&text).unwrap_or_default();
            if let Some(ctrl) = root
                .as_mapping()
                .and_then(|m| m.get(&serde_yaml::Value::String("external-controller".into())))
                .and_then(|v| v.as_str())
            {
                return Ok(ctrl.to_string());
            }
        }
    }
    Ok(constants::DEFAULT_CONTROLLER.to_string())
}

/// 模型选择：如果用户明确指定了 --model 则直接使用，否则尝试交互选择
fn select_model(current_model: Option<&str>, api_base: &str, api_key: &str) -> Result<String> {
    if let Some(model) = current_model {
        return Ok(model.to_string());
    }

    let default_model = "gpt-4o";

    if is_json_mode() || !std::io::stdin().is_terminal() {
        return Ok(default_model.to_string());
    }

    let client = match ai_protocol::build_llm_client() {
        Ok(c) => c,
        Err(_) => return Ok(default_model.to_string()),
    };

    let models = match ai_protocol::list_models(&client, api_base, api_key) {
        Ok(m) if !m.is_empty() => m,
        _ => return Ok(default_model.to_string()),
    };

    println!("可用模型 ({} 个):", models.len());
    for (i, name) in models.iter().enumerate() {
        println!("  {:>3}. {}", i + 1, name);
    }
    println!();
    println!("输入序号选择模型（直接回车使用默认 {}）:", default_model);

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Ok(default_model.to_string());
    }

    if let Ok(idx) = trimmed.parse::<usize>() {
        if idx >= 1 && idx <= models.len() {
            return Ok(models[idx - 1].clone());
        }
    }

    // 输入的可能是模型名
    Ok(trimmed.to_string())
}

fn truncate_args(s: &str) -> String {
    if s.len() <= 80 {
        s.to_string()
    } else {
        format!("{}...", &s[..80])
    }
}
