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
use crate::mixin;
use crate::output::{is_machine_mode, print_machine};
use crate::paths::app_paths;
use crate::profile;
use crate::utils::{self, command_exists, ensure_supported_host, normalize_unit_name, now_unix};

pub use self::checks::{DoctorCheckView, docker_bridge_dataplane_note};
use self::config::*;
pub use self::config::{apply_tun_policy_overlay, apply_tun_policy_overlay_for};
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
    ensure_supported_host()?;
    if ensure_tun_privileges_or_delegate(TunAction::On, &args)? == PrivilegeCheck::Delegated {
        return Ok(());
    }
    let json_mode = is_machine_mode();

    if utils::is_linux() && !Path::new("/dev/net/tun").exists() {
        bail!("未找到 /dev/net/tun，请先修复系统环境");
    }

    let paths = app_paths()?;
    let mut overlay = mixin::load_mixin_or_empty(&paths.profile_mixin_file)?;
    apply_tun_policy_overlay(&mut overlay, true);

    let auto_redirect = bool_field(key_value(&overlay, "tun"), "auto-redirect").unwrap_or(false);
    let redir_port = u16_field(Some(&overlay), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);

    mixin::save_mixin(&paths.profile_mixin_file, &overlay)?;
    profile::render_runtime_from_home(&paths)?;

    // mihomo 在 auto-redirect=true 时自行管理 nft 规则，
    // clash CLI 不再自建规则，避免双表冲突。
    // 无论 auto_redirect 与否，都清理可能存在的历史自建规则。
    if utils::is_linux() {
        cleanup_dataplane_rules_all_best_effort();
    }
    if !json_mode {
        if utils::is_macos() {
            println!("macOS TUN 使用 mihomo auto-route / utun，不经过 nft auto-redirect。");
        } else if auto_redirect {
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
        return print_machine(&serde_json::json!({
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

    println!(
        "已将 tun 策略写入 mixin 并渲染运行配置: {}",
        paths.runtime_config_file.display()
    );
    if args.no_restart {
        println!("已跳过服务重启（--no-restart）。");
    }
    println!("建议执行 `clash tun doctor` 复检环境。");
    Ok(())
}

fn cmd_off(args: TunApplyArgs) -> Result<()> {
    ensure_supported_host()?;
    if ensure_tun_privileges_or_delegate(TunAction::Off, &args)? == PrivilegeCheck::Delegated {
        return Ok(());
    }
    let json_mode = is_machine_mode();
    let paths = app_paths()?;
    let mut overlay = mixin::load_mixin_or_empty(&paths.profile_mixin_file)?;
    apply_tun_policy_overlay(&mut overlay, false);
    let redir_port = u16_field(Some(&overlay), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);
    mixin::save_mixin(&paths.profile_mixin_file, &overlay)?;
    profile::render_runtime_from_home(&paths)?;

    let previous_state = read_tun_state(&paths.runtime_tun_state_file)?;
    cleanup_linux_dataplane_after_tun_off(previous_state.as_ref()).context("清理数据面规则失败")?;

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
        return print_machine(&serde_json::json!({
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
    ensure_supported_host()?;
    let paths = app_paths()?;
    let root = if paths.runtime_config_file.exists() {
        load_existing_config(&paths.runtime_config_file)?
    } else {
        if !is_machine_mode() {
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

    if !is_machine_mode() {
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

    let device_ok = tun_device_supported();
    // macOS 的 utun 编号动态分配，仅凭“服务已加载”不能证明 TUN 接口属于 mihomo。
    // 因此这里明确返回未知，而不是把未验证状态当成 true。
    let interface_ready = if utils::is_macos() {
        None
    } else {
        Some(tun_interface_ready(tun))
    };
    let backend_installed = command_exists("nft") || command_exists("iptables");
    let active_backend = detect_active_rule_backend();
    let rules_active = active_backend != RuleBackend::None;
    let service_active = query_service_active(&args.name, args.user).unwrap_or(false);
    let last_state = read_tun_state(&paths.runtime_tun_state_file)?;
    // 数据面由内核 auto-route / auto-redirect 管理，CLI 自建表缺失不视为失败。
    let actual_ok = interface_ready.map(|ready| actual_tun_ok(tun_enable, ready, service_active));

    if is_machine_mode() {
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
        return print_machine(&serde_json::json!({
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
                "interface_ready": interface_ready,
                "interface_verified": interface_ready.is_some(),
                "interface": expected_tun_interface(tun),
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

    let interface_text = interface_ready
        .map(yes_no)
        .unwrap_or("未确认（macOS 动态 utun）");
    println!(
        "系统能力: /dev/net/tun={}, TUN 接口={} ({}), backend={}",
        yes_no(device_ok),
        interface_text,
        expected_tun_interface(tun),
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

    match actual_ok {
        Some(true) => println!("实际状态: 生效"),
        Some(false) => {
            println!("实际状态: 未生效");
            println!("建议执行 `clash tun doctor` 查看详细问题。");
        }
        None => println!("实际状态: 未确认（macOS 无法仅凭 launchd 状态确认动态 utun 接管）"),
    }
    Ok(())
}

/// 实际是否接管：配置开启 + 运行时 TUN 接口存在 + 服务在跑。
/// 不要求 CLI 自建 nft/iptables 表。
pub fn actual_tun_ok(tun_enable: bool, interface_ready: bool, service_active: bool) -> bool {
    tun_enable && interface_ready && service_active
}

fn tun_device_supported() -> bool {
    if utils::is_macos() {
        true
    } else {
        Path::new("/dev/net/tun").exists()
    }
}

fn expected_tun_interface(tun: Option<&Value>) -> String {
    string_field(tun, "device").unwrap_or_else(|| "Meta".to_string())
}

fn tun_interface_ready(tun: Option<&Value>) -> bool {
    if utils::is_macos() {
        // macOS 的 utun 编号由系统动态分配，服务状态仍由 launchd 检查。
        return true;
    }
    if !tun_device_supported() {
        return false;
    }
    let interface = expected_tun_interface(tun);
    if interface.is_empty() || interface.contains('/') {
        return false;
    }
    Path::new("/sys/class/net")
        .join(interface)
        .join("tun_flags")
        .exists()
}

/// Linux nft/iptables 历史规则清理。macOS 走 utun/auto-route，没有这些表。
fn cleanup_linux_dataplane_after_tun_off(previous: Option<&TunState>) -> Result<()> {
    if !utils::is_linux() {
        return Ok(());
    }
    if let Some(state) = previous {
        if state.rules_applied {
            return cleanup_dataplane_rules(state.backend);
        }
        return Ok(());
    }
    cleanup_dataplane_rules_all()
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
    match crate::service::restart_managed_service(name, user) {
        Ok(()) => {
            if !is_machine_mode() {
                println!("已重启服务: {}", name);
            }
            true
        }
        Err(err) => {
            if !is_machine_mode() {
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
    crate::service::is_managed_service_active(name, user)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_ok_does_not_require_cli_owned_tables() {
        assert!(actual_tun_ok(true, true, true));
        assert!(!actual_tun_ok(false, true, true));
        assert!(!actual_tun_ok(true, false, true));
        assert!(!actual_tun_ok(true, true, false));
    }

    #[test]
    fn tun_interface_name_defaults_to_meta_and_honors_config() {
        let root = Value::Mapping(Mapping::new());
        assert_eq!(expected_tun_interface(Some(&root)), "Meta");

        let mut root = Value::Mapping(Mapping::new());
        config::set_default_string_field(&mut root, &[], "device", "mihomo-test");
        assert_eq!(expected_tun_interface(Some(&root)), "mihomo-test");
    }

    #[test]
    fn dataplane_cleanup_is_noop_off_linux() {
        if cfg!(target_os = "linux") {
            return;
        }
        cleanup_linux_dataplane_after_tun_off(None).unwrap();
    }
}
