use std::io::IsTerminal;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::ai_protocol::{
    self, LlmConfig, LlmResponse, Protocol, assistant_tool_calls_message,
    tool_result_message_completions, tool_result_message_responses,
};
use crate::ai_tools::{self, MihomoCtx};
use crate::cli::{AiCommand, AiRulesArgs};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

pub fn run(command: AiCommand) -> Result<()> {
    match command {
        AiCommand::Rules(args) => cmd_rules(args),
        AiCommand::Models(args) => cmd_models(args),
    }
}

fn cmd_models(args: crate::cli::AiModelsArgs) -> Result<()> {
    let api_key = resolve_api_key(&args.api_key)?;
    let client = ai_protocol::build_llm_client()?;
    let models = ai_protocol::list_models(&client, &args.api_base, &api_key)?;

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
    let api_key = resolve_api_key(&args.api_key)?;
    let protocol = match args.protocol.as_str() {
        "responses" => Protocol::Responses,
        _ => Protocol::Completions,
    };

    let model = select_model(&args)?;

    let config = LlmConfig {
        api_base: args.api_base.clone(),
        api_key,
        model: model.clone(),
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

    // 初始用户消息
    messages.push(serde_json::json!({
        "role": "user",
        "content": "请分析当前的连接情况和路由规则，找出配置不合理的地方，并给出优化建议。如果我没有使用 --dry-run 模式，你可以直接修改规则并验证效果。",
    }));

    if !is_json_mode() {
        println!("AI 规则分析启动（模型: {}，协议: {:?}）", model, protocol);
        if args.dry_run {
            println!("[dry-run 模式] 写操作将模拟执行，不会实际修改配置");
        }
        println!("---");
    }

    // Agent Loop
    let max_turns = args.max_turns;
    for turn in 0..max_turns {
        let response = ai_protocol::call_llm(&client, &config, &messages, &tools)
            .with_context(|| format!("第 {} 轮 LLM 调用失败", turn + 1))?;

        match response {
            LlmResponse::Text(text) => {
                if is_json_mode() {
                    return print_json(&serde_json::json!({
                        "ok": true,
                        "action": "ai.rules",
                        "turns": turn + 1,
                        "result": text,
                    }));
                }
                println!("{}", text);
                return Ok(());
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
                messages.push(assistant_tool_calls_message(&tool_calls));

                // 执行每个工具调用并将结果加入历史
                for tc in &tool_calls {
                    let result =
                        ai_tools::execute_tool(&tc.name, &tc.arguments, &mihomo, args.dry_run);

                    let result_msg = match protocol {
                        Protocol::Completions => tool_result_message_completions(&tc.id, &result),
                        Protocol::Responses => tool_result_message_responses(&tc.id, &result),
                    };
                    messages.push(result_msg);
                }
            }
        }
    }

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": false,
            "action": "ai.rules",
            "error": format!("达到最大轮次限制 ({max_turns})"),
        }));
    }

    println!("---");
    println!("达到最大轮次限制 ({})，AI 分析结束。", max_turns);
    Ok(())
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

fn resolve_api_key(explicit: &Option<String>) -> Result<String> {
    if let Some(key) = explicit {
        return Ok(key.clone());
    }
    std::env::var("OPENAI_API_KEY")
        .context("未提供 API Key。请设置 OPENAI_API_KEY 环境变量或使用 --api-key 参数")
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
    Ok("127.0.0.1:9090".to_string())
}

/// 模型选择：如果用户明确指定了 --model 则直接使用，否则尝试交互选择
fn select_model(args: &AiRulesArgs) -> Result<String> {
    // 如果用户明确指定了非默认模型，直接用
    if args.model != "gpt-4o" {
        return Ok(args.model.clone());
    }

    // 非交互终端或 json 模式，直接用默认
    if is_json_mode() || !std::io::stdin().is_terminal() {
        return Ok(args.model.clone());
    }

    // 尝试获取模型列表供交互选择
    let api_key = match resolve_api_key(&args.api_key) {
        Ok(k) => k,
        Err(_) => return Ok(args.model.clone()),
    };

    let client = match ai_protocol::build_llm_client() {
        Ok(c) => c,
        Err(_) => return Ok(args.model.clone()),
    };

    let models = match ai_protocol::list_models(&client, &args.api_base, &api_key) {
        Ok(m) if !m.is_empty() => m,
        _ => return Ok(args.model.clone()),
    };

    println!("可用模型 ({} 个):", models.len());
    for (i, name) in models.iter().enumerate() {
        println!("  {:>3}. {}", i + 1, name);
    }
    println!();
    println!("输入序号选择模型（直接回车使用默认 {}）:", args.model);

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Ok(args.model.clone());
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
