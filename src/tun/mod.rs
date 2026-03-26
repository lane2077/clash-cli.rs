mod checks;
mod config;
mod detect;
mod privilege;
mod rules;
mod state;

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_yaml::{Mapping, Value};

use crate::cli::{TunApplyArgs, TunCommand, TunStatusArgs};
use crate::constants::DEFAULT_REDIR_PORT;
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;
use crate::utils::{self, command_exists, ensure_linux_host, normalize_unit_name, now_unix};

use self::config::*;
use self::detect::detect_exclude_uids;
use self::privilege::{PrivilegeCheck, TunAction, ensure_tun_privileges_or_delegate};
use self::rules::{
    cleanup_dataplane_rules, cleanup_dataplane_rules_all, cleanup_dataplane_rules_all_best_effort,
    detect_active_rule_backend,
};
use self::state::{RuleBackend, TunState, read_tun_state, write_tun_state};

pub fn run(command: TunCommand) -> Result<()> {
    match command {
        TunCommand::Doctor => checks::cmd_doctor(),
        TunCommand::On(args) => cmd_on(args),
        TunCommand::Off(args) => cmd_off(args),
        TunCommand::Status(args) => cmd_status(args),
    }
}

fn cmd_on(args: TunApplyArgs) -> Result<()> {
    ensure_linux_host()?;
    if ensure_tun_privileges_or_delegate(TunAction::On, &args)? == PrivilegeCheck::Delegated {
        return Ok(());
    }
    let json_mode = is_json_mode();

    if !Path::new("/dev/net/tun").exists() {
        bail!("未找到 /dev/net/tun，请先修复系统环境");
    }

    let paths = app_paths()?;
    let mut root = load_or_init_config(&paths.runtime_config_file)?;

    set_bool_field(&mut root, &["tun"], "enable", true);
    set_default_bool_field(&mut root, &["tun"], "auto-route", true);
    set_default_bool_field(&mut root, &["tun"], "auto-detect-interface", true);
    set_default_bool_field(&mut root, &["tun"], "auto-redirect", true);
    // strict-route 在部分发行版/路由环境下更容易触发 /1 路由写入失败，默认保守关闭。
    set_default_bool_field(&mut root, &["tun"], "strict-route", false);
    set_default_string_field(&mut root, &["tun"], "stack", "mixed");
    set_default_bool_field(&mut root, &["dns"], "enable", true);
    // Linux 桌面环境下优先稳定性：默认关闭 IPv6，避免常见的 auto-route 路由下发失败。
    set_bool_field(&mut root, &[], "ipv6", false);
    set_bool_field(&mut root, &["dns"], "ipv6", false);
    set_default_string_field(&mut root, &["dns"], "enhanced-mode", "fake-ip");
    // dns-hijack 确保系统 DNS 请求被 mihomo 接管，fake-ip 模式必需。
    set_default_sequence_field(&mut root, &["tun"], "dns-hijack", &["any:53".to_string()]);
    set_default_u16_field(&mut root, &[], "redir-port", DEFAULT_REDIR_PORT);

    // mihomo 的 auto-redirect 自己能正确处理 Docker 流量，不需要接口排除
    remove_tun_key(&mut root, "include-interface");
    remove_tun_key(&mut root, "exclude-interface");
    // 检测需要排除的 UID（cloudflared 等服务进程）
    let excluded_uids = detect_exclude_uids();
    if !excluded_uids.is_empty() {
        let uid_values: Vec<String> = excluded_uids.iter().map(|u| u.to_string()).collect();
        set_u32_sequence_field(&mut root, &["tun"], "exclude-uid", &excluded_uids);
        if !json_mode {
            println!("已检测到排除 UID: {}", uid_values.join(", "));
        }
    }

    let auto_redirect = bool_field(key_value(&root, "tun"), "auto-redirect").unwrap_or(false);
    let redir_port = u16_field(Some(&root), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);

    save_config(&paths.runtime_config_file, &root)?;

    // mihomo 在 auto-redirect=true 时自行管理 nft 规则，
    // clash CLI 不再自建规则，避免双表冲突。
    // 无论 auto_redirect 与否，都清理可能存在的历史自建规则。
    cleanup_dataplane_rules_all_best_effort();
    if !json_mode {
        if auto_redirect {
            println!("已配置 auto-redirect=true，mihomo 将自行管理数据面规则");
        } else {
            println!("检测到 tun.auto-redirect=false，已跳过规则下发。");
        }
    }
    let backend = RuleBackend::None;
    let rules_applied = false;

    write_tun_state(
        &paths.runtime_tun_state_file,
        TunState {
            enabled: true,
            service_name: args.name.clone(),
            user_service: args.user,
            backend,
            redir_port,
            rules_applied,
            updated_at: now_unix(),
        },
    )?;

    let restart_attempted = !args.no_restart;
    let restart_ok = if args.no_restart {
        None
    } else {
        Some(restart_service_best_effort(&args.name, args.user))
    };

    if json_mode {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "tun.on",
            "config_path": paths.runtime_config_file.display().to_string(),
            "service": normalize_unit_name(&args.name),
            "user_service": args.user,
            "backend": backend.as_str(),
            "redir_port": redir_port,
            "rules_applied": rules_applied,
            "restart_attempted": restart_attempted,
            "restart_ok": restart_ok
        }));
    }

    println!("已开启 tun 配置: {}", paths.runtime_config_file.display());
    if args.no_restart {
        println!("已跳过服务重启（--no-restart）。");
    }
    println!("建议执行 `clash tun doctor` 复检环境。");
    Ok(())
}

fn cmd_off(args: TunApplyArgs) -> Result<()> {
    ensure_linux_host()?;
    if ensure_tun_privileges_or_delegate(TunAction::Off, &args)? == PrivilegeCheck::Delegated {
        return Ok(());
    }
    let json_mode = is_json_mode();
    let paths = app_paths()?;
    let mut root = load_or_init_config(&paths.runtime_config_file)?;

    let previous_state = read_tun_state(&paths.runtime_tun_state_file)?;
    let redir_port = u16_field(Some(&root), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);

    set_bool_field(&mut root, &["tun"], "enable", false);
    save_config(&paths.runtime_config_file, &root)?;

    let cleanup_result = if let Some(state) = previous_state.as_ref() {
        if state.rules_applied {
            cleanup_dataplane_rules(state.backend)
        } else {
            Ok(())
        }
    } else {
        cleanup_dataplane_rules_all()
    };
    cleanup_result.context("清理数据面规则失败")?;

    write_tun_state(
        &paths.runtime_tun_state_file,
        TunState {
            enabled: false,
            service_name: args.name.clone(),
            user_service: args.user,
            backend: RuleBackend::None,
            redir_port,
            rules_applied: false,
            updated_at: now_unix(),
        },
    )?;

    let restart_attempted = !args.no_restart;
    let restart_ok = if args.no_restart {
        None
    } else {
        Some(restart_service_best_effort(&args.name, args.user))
    };

    if json_mode {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "tun.off",
            "config_path": paths.runtime_config_file.display().to_string(),
            "service": normalize_unit_name(&args.name),
            "user_service": args.user,
            "redir_port": redir_port,
            "restart_attempted": restart_attempted,
            "restart_ok": restart_ok
        }));
    }

    println!("已关闭 tun 配置并清理数据面规则。");
    if args.no_restart {
        println!("已跳过服务重启（--no-restart）。");
    }
    Ok(())
}

fn cmd_status(args: TunStatusArgs) -> Result<()> {
    ensure_linux_host()?;
    let paths = app_paths()?;
    let root = if paths.runtime_config_file.exists() {
        load_existing_config(&paths.runtime_config_file)?
    } else {
        if !is_json_mode() {
            println!(
                "未找到运行配置文件，将按未配置状态展示: {}",
                paths.runtime_config_file.display()
            );
        }
        Value::Mapping(Mapping::new())
    };

    let tun = key_value(&root, "tun");
    let dns = key_value(&root, "dns");
    let tun_enable = bool_field(tun, "enable").unwrap_or(false);
    let _auto_redirect = bool_field(tun, "auto-redirect").unwrap_or(false);
    let redir_port = u16_field(Some(&root), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);

    if !is_json_mode() {
        println!("tun 配置文件: {}", paths.runtime_config_file.display());
        println!("配置状态: {}", if tun_enable { "已开启" } else { "已关闭" });
        println!("redir-port: {}", redir_port);
        println!(
            "tun.auto-route: {}",
            bool_or_unset(bool_field(tun, "auto-route"))
        );
        println!(
            "tun.auto-redirect: {}",
            bool_or_unset(bool_field(tun, "auto-redirect"))
        );
        println!(
            "tun.strict-route: {}",
            bool_or_unset(bool_field(tun, "strict-route"))
        );
        println!(
            "tun.stack: {}",
            string_field(tun, "stack").unwrap_or_else(|| "未配置".to_string())
        );
        println!("dns.enable: {}", bool_or_unset(bool_field(dns, "enable")));
        println!(
            "dns.enhanced-mode: {}",
            string_field(dns, "enhanced-mode").unwrap_or_else(|| "未配置".to_string())
        );
        println!("ipv6: {}", bool_or_unset(bool_field(Some(&root), "ipv6")));
        println!("dns.ipv6: {}", bool_or_unset(bool_field(dns, "ipv6")));
    }

    let device_ok = Path::new("/dev/net/tun").exists();
    let backend_installed = command_exists("nft") || command_exists("iptables");
    let active_backend = detect_active_rule_backend();
    let rules_active = active_backend != RuleBackend::None;
    // auto-redirect 由 mihomo 管理，不再检查自建规则是否存在
    let redirect_ready = true;
    let service_active = query_service_active(&args.name, args.user).unwrap_or(false);
    let last_state = read_tun_state(&paths.runtime_tun_state_file)?;
    let actual_ok = tun_enable && device_ok && redirect_ready && service_active;

    if is_json_mode() {
        let last_state_json = match last_state {
            Some(state) => serde_json::json!({
                "enabled": state.enabled,
                "service_name": state.service_name,
                "user_service": state.user_service,
                "backend": state.backend.as_str(),
                "redir_port": state.redir_port,
                "rules_applied": state.rules_applied,
                "updated_at": state.updated_at
            }),
            None => serde_json::Value::Null,
        };
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "tun.status",
            "config": {
                "path": paths.runtime_config_file.display().to_string(),
                "tun_enable": tun_enable,
                "redir_port": redir_port,
                "tun_auto_route": bool_field(tun, "auto-route"),
                "tun_auto_redirect": bool_field(tun, "auto-redirect"),
                "tun_strict_route": bool_field(tun, "strict-route"),
                "tun_stack": string_field(tun, "stack"),
                "dns_enable": bool_field(dns, "enable"),
                "dns_enhanced_mode": string_field(dns, "enhanced-mode"),
                "ipv6": bool_field(Some(&root), "ipv6"),
                "dns_ipv6": bool_field(dns, "ipv6"),
            },
            "runtime": {
                "device_ok": device_ok,
                "backend_installed": backend_installed,
                "active_backend": active_backend.as_str(),
                "rules_active": rules_active,
                "service_active": service_active,
                "service": normalize_unit_name(&args.name),
                "user_service": args.user
            },
            "last_state": last_state_json,
            "actual_ok": actual_ok
        }));
    }

    println!(
        "系统能力: /dev/net/tun={}, backend={}",
        yes_no(device_ok),
        yes_no(backend_installed)
    );
    println!(
        "数据面规则: {} ({})",
        if rules_active {
            "已生效"
        } else {
            "未生效"
        },
        active_backend.as_str()
    );
    println!(
        "服务状态({}): {}",
        normalize_unit_name(&args.name),
        if service_active {
            "运行中"
        } else {
            "未运行"
        }
    );

    match last_state {
        Some(state) => println!(
            "最近操作: enabled={}, backend={}, rules_applied={}, service={}, user={}, ts={}",
            state.enabled,
            state.backend.as_str(),
            state.rules_applied,
            state.service_name,
            state.user_service,
            state.updated_at
        ),
        None => println!("最近操作: 无"),
    }

    println!("实际状态: {}", if actual_ok { "生效" } else { "未生效" });
    if !actual_ok {
        println!("建议执行 `clash tun doctor` 查看详细问题。");
    }
    Ok(())
}

// --- Shared helpers used across submodules ---

pub(super) fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("执行命令失败: {} {:?}", program, args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "命令执行失败: {} {:?}\nstdout: {}\nstderr: {}",
            program,
            args,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(())
}

fn restart_service_best_effort(name: &str, user: bool) -> bool {
    let mut args = vec![];
    if user {
        args.push("--user");
    }
    args.push("restart");
    let unit = normalize_unit_name(name);
    args.push(unit.as_str());
    match run_cmd("systemctl", &args) {
        Ok(()) => {
            if !is_json_mode() {
                println!("已重启服务: {}", normalize_unit_name(name));
            }
            true
        }
        Err(err) => {
            if !is_json_mode() {
                eprintln!("警告: 自动重启服务失败: {}", err);
                eprintln!(
                    "请手动执行: clash service restart --name {}{}",
                    name,
                    if user { " --user" } else { "" }
                );
            }
            false
        }
    }
}

fn query_service_active(name: &str, user: bool) -> Result<bool> {
    if !utils::command_exists("systemctl") {
        bail!("未检测到 systemctl");
    }
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    let unit = normalize_unit_name(name);
    let status = cmd.arg("is-active").arg("--quiet").arg(unit).status();
    match status {
        Ok(v) => Ok(v.success()),
        Err(err) => Err(err).context("执行 systemctl is-active 失败"),
    }
}

fn bool_or_unset(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "未配置",
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "是" } else { "否" }
}
