use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_yaml::{Mapping, Number, Value};

use crate::cli::{TunApplyArgs, TunCommand, TunStatusArgs};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

const CAP_NET_ADMIN_BIT: u32 = 12;
const CAP_NET_RAW_BIT: u32 = 13;
const DEFAULT_REDIR_PORT: u16 = 7892;
const NFT_TABLE_NAME: &str = "clash_cli_tun";
const IPT_CHAIN_NAME: &str = "CLASH_CLI_TUN";

#[derive(Clone, Copy, Debug)]
enum CheckLevel {
    Pass,
    Warn,
    Fail,
}

struct CheckItem {
    name: &'static str,
    level: CheckLevel,
    message: String,
    suggestion: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleBackend {
    Nft,
    Iptables,
    None,
}

impl RuleBackend {
    fn as_str(self) -> &'static str {
        match self {
            RuleBackend::Nft => "nft",
            RuleBackend::Iptables => "iptables",
            RuleBackend::None => "none",
        }
    }

    fn from_str(v: &str) -> Self {
        match v {
            "nft" => RuleBackend::Nft,
            "iptables" => RuleBackend::Iptables,
            _ => RuleBackend::None,
        }
    }
}

#[derive(Debug)]
struct TunState {
    enabled: bool,
    service_name: String,
    user_service: bool,
    backend: RuleBackend,
    redir_port: u16,
    rules_applied: bool,
    updated_at: u64,
}

impl TunState {
    fn to_text(&self) -> String {
        format!(
            "enabled={}\nservice_name={}\nuser_service={}\nbackend={}\nredir_port={}\nrules_applied={}\nupdated_at={}\n",
            self.enabled,
            self.service_name,
            self.user_service,
            self.backend.as_str(),
            self.redir_port,
            self.rules_applied,
            self.updated_at
        )
    }

    fn from_text(content: &str) -> Result<Self> {
        let mut enabled = None;
        let mut service_name = None;
        let mut user_service = None;
        let mut backend = None;
        let mut redir_port = None;
        let mut rules_applied = None;
        let mut updated_at = None;

        for line in content.lines() {
            let mut parts = line.splitn(2, '=');
            let key = parts.next().unwrap_or_default().trim();
            let value = parts.next().unwrap_or_default().trim();
            match key {
                "enabled" => enabled = Some(value == "true"),
                "service_name" => service_name = Some(value.to_string()),
                "user_service" => user_service = Some(value == "true"),
                "backend" => backend = Some(RuleBackend::from_str(value)),
                "redir_port" => {
                    redir_port = Some(
                        value
                            .parse::<u16>()
                            .context("解析 tun.state.redir_port 失败")?,
                    )
                }
                "rules_applied" => rules_applied = Some(value == "true"),
                "updated_at" => {
                    updated_at = Some(
                        value
                            .parse::<u64>()
                            .context("解析 tun.state.updated_at 失败")?,
                    )
                }
                _ => {}
            }
        }

        Ok(Self {
            enabled: enabled.context("tun.state 缺少 enabled")?,
            service_name: service_name.context("tun.state 缺少 service_name")?,
            user_service: user_service.context("tun.state 缺少 user_service")?,
            backend: backend.unwrap_or(RuleBackend::None),
            redir_port: redir_port.unwrap_or(DEFAULT_REDIR_PORT),
            rules_applied: rules_applied.unwrap_or(false),
            updated_at: updated_at.context("tun.state 缺少 updated_at")?,
        })
    }
}

pub fn run(command: TunCommand) -> Result<()> {
    match command {
        TunCommand::Doctor => cmd_doctor(),
        TunCommand::On(args) => cmd_on(args),
        TunCommand::Off(args) => cmd_off(args),
        TunCommand::Status(args) => cmd_status(args),
    }
}

fn cmd_doctor() -> Result<()> {
    ensure_linux_host()?;
    if ensure_tun_doctor_privileges_or_delegate()? == PrivilegeCheck::Delegated {
        return Ok(());
    }
    if !is_json_mode() {
        println!("开始执行 tun 诊断...");
    }

    let paths = app_paths()?;
    let mut checks = vec![
        check_tun_device(),
        check_capability(
            CAP_NET_ADMIN_BIT,
            "CAP_NET_ADMIN",
            "建议使用 systemd AmbientCapabilities 或 root 运行",
        ),
        check_capability(
            CAP_NET_RAW_BIT,
            "CAP_NET_RAW",
            "建议补充 CAP_NET_RAW，避免部分流量处理受限",
        ),
        check_backend(),
        check_sysctl_value(
            "/proc/sys/net/ipv4/ip_forward",
            "内核转发(net.ipv4.ip_forward)",
            "1",
            "可执行: sudo sysctl -w net.ipv4.ip_forward=1",
        ),
        check_rp_filter(),
    ];

    let (config_checks, config_tun_enable, config_auto_redirect) =
        check_config(&paths.runtime_config_file);
    checks.extend(config_checks);

    if config_tun_enable && config_auto_redirect {
        let active_backend = detect_active_rule_backend();
        if active_backend == RuleBackend::None {
            checks.push(warn(
                "数据面规则",
                "配置要求 auto-redirect，但当前未检测到本工具管理的规则",
                "可执行 `clash tun on` 重新下发规则",
            ));
        } else {
            checks.push(pass(
                "数据面规则",
                &format!("检测到 {} 规则已存在", active_backend.as_str()),
            ));
        }
    }

    let (pass_count, warn_count, fail_count) = if is_json_mode() {
        summarize_checks(&checks)
    } else {
        print_checks(&checks)
    };

    if is_json_mode() {
        let list = checks
            .iter()
            .map(|item| {
                serde_json::json!({
                    "name": item.name,
                    "level": check_level_str(item.level),
                    "message": item.message,
                    "suggestion": item.suggestion
                })
            })
            .collect::<Vec<_>>();
        return print_json(&serde_json::json!({
            "ok": fail_count == 0,
            "action": "tun.doctor",
            "summary": {
                "pass": pass_count,
                "warn": warn_count,
                "fail": fail_count
            },
            "checks": list
        }));
    }

    println!();
    println!(
        "诊断汇总: PASS={} WARN={} FAIL={}",
        pass_count, warn_count, fail_count
    );

    if fail_count > 0 {
        bail!("tun 诊断未通过，请先处理 FAIL 项");
    }

    if warn_count > 0 {
        println!("tun 诊断通过，但存在 WARN 项，建议按提示优化。");
    } else {
        println!("tun 诊断通过，当前环境可用于 tun 模式。");
    }
    Ok(())
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
    let original_root = root.clone();

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
    set_default_u16_field(&mut root, &[], "redir-port", DEFAULT_REDIR_PORT);

    let auto_redirect = bool_field(key_value(&root, "tun"), "auto-redirect").unwrap_or(false);
    let redir_port = u16_field(Some(&root), "redir-port").unwrap_or(DEFAULT_REDIR_PORT);

    save_config(&paths.runtime_config_file, &root)?;

    let (backend, rules_applied) = if auto_redirect {
        let preferred_backend = select_rule_backend()?;
        let backend = match apply_dataplane_rules(preferred_backend, redir_port) {
            Ok(actual_backend) => actual_backend,
            Err(err) => {
                save_config(&paths.runtime_config_file, &original_root)?;
                if json_mode {
                    return print_json(&serde_json::json!({
                        "ok": false,
                        "action": "tun.on",
                        "error": err.to_string(),
                        "rolled_back": true
                    }));
                }
                eprintln!("错误: 下发数据面规则失败: {}", err);
                eprintln!("已回滚 tun 配置到启用前状态。");
                bail!("tun 开启失败");
            }
        };
        if !json_mode {
            println!(
                "已下发 {} 数据面规则，redir-port={}",
                backend.as_str(),
                redir_port
            );
        }
        (backend, true)
    } else {
        cleanup_dataplane_rules_all_best_effort();
        if !json_mode {
            println!("检测到 tun.auto-redirect=false，已跳过规则下发。");
        }
        (RuleBackend::None, false)
    };

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
    let auto_redirect = bool_field(tun, "auto-redirect").unwrap_or(false);
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
    let redirect_ready = if auto_redirect { rules_active } else { true };
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

fn check_tun_device() -> CheckItem {
    if !Path::new("/dev/net/tun").exists() {
        return fail(
            "TUN 设备(/dev/net/tun)",
            "未找到设备节点",
            "请确认内核支持 TUN，并加载 tun 模块",
        );
    }
    pass("TUN 设备(/dev/net/tun)", "设备节点存在")
}

fn check_capability(bit: u32, cap_name: &'static str, suggestion: &'static str) -> CheckItem {
    match read_cap_eff() {
        Ok(mask) => {
            if (mask & (1u64 << bit)) != 0 {
                pass(cap_name, "当前进程具备能力")
            } else {
                fail(cap_name, "当前进程缺少能力", suggestion)
            }
        }
        Err(err) => warn(
            cap_name,
            &format!("无法检测 capability: {err}"),
            "可手动检查 /proc/self/status 的 CapEff 字段",
        ),
    }
}

fn check_backend() -> CheckItem {
    if command_exists("nft") {
        return pass("防火墙后端", "检测到 nft，可优先使用 nftables");
    }
    if command_exists("iptables") {
        return warn(
            "防火墙后端",
            "未检测到 nft，回退使用 iptables",
            "建议安装 nftables，后续 tun 规则管理更稳",
        );
    }
    fail(
        "防火墙后端",
        "未检测到 nft/iptables",
        "请安装 nftables 或 iptables",
    )
}

fn check_sysctl_value(
    path: &str,
    name: &'static str,
    expected: &str,
    suggestion: &'static str,
) -> CheckItem {
    match fs::read_to_string(path) {
        Ok(value) => {
            let current = value.trim();
            if current == expected {
                pass(name, &format!("当前值={current}"))
            } else {
                warn(
                    name,
                    &format!("当前值={current}，期望值={expected}"),
                    suggestion,
                )
            }
        }
        Err(err) => warn(name, &format!("读取失败: {err}"), "请手动检查 sysctl 参数"),
    }
}

fn check_rp_filter() -> CheckItem {
    let path = "/proc/sys/net/ipv4/conf/all/rp_filter";
    match fs::read_to_string(path) {
        Ok(value) => {
            let current = value.trim();
            if current == "0" || current == "2" {
                pass(
                    "反向路径过滤(net.ipv4.conf.all.rp_filter)",
                    &format!("当前值={current}"),
                )
            } else {
                warn(
                    "反向路径过滤(net.ipv4.conf.all.rp_filter)",
                    &format!("当前值={current}，建议设置为 0 或 2"),
                    "可执行: sudo sysctl -w net.ipv4.conf.all.rp_filter=0",
                )
            }
        }
        Err(err) => warn(
            "反向路径过滤(net.ipv4.conf.all.rp_filter)",
            &format!("读取失败: {err}"),
            "请手动检查 rp_filter",
        ),
    }
}

fn check_config(config_path: &Path) -> (Vec<CheckItem>, bool, bool) {
    if !config_path.exists() {
        return (
            vec![warn(
                "运行配置(runtime/config.yaml)",
                &format!("未找到配置文件: {}", config_path.display()),
                "先执行 `clash service install` 生成模板配置",
            )],
            false,
            false,
        );
    }

    let root = match load_existing_config(config_path) {
        Ok(v) => v,
        Err(err) => {
            return (
                vec![fail(
                    "运行配置(runtime/config.yaml)",
                    &format!("读取失败: {err}"),
                    "请检查配置文件权限与 YAML 格式",
                )],
                false,
                false,
            );
        }
    };

    let tun = key_value(&root, "tun");
    let dns = key_value(&root, "dns");
    let tun_enable = bool_field(tun, "enable").unwrap_or(false);
    let auto_route = bool_field(tun, "auto-route");
    let auto_redirect = bool_field(tun, "auto-redirect").unwrap_or(false);
    let strict_route = bool_field(tun, "strict-route");
    let auto_detect_interface = bool_field(tun, "auto-detect-interface");
    let tun_stack = string_field(tun, "stack");
    let dns_enable = bool_field(dns, "enable");
    let dns_mode = string_field(dns, "enhanced-mode");

    let mut items = Vec::new();
    items.push(if tun_enable {
        pass("tun.enable", "已开启")
    } else {
        fail("tun.enable", "未开启", "请设置 tun.enable: true")
    });
    items.push(match auto_route {
        Some(true) => pass("tun.auto-route", "已开启"),
        Some(false) => warn(
            "tun.auto-route",
            "已关闭",
            "建议开启 auto-route，避免手工维护路由",
        ),
        None => warn(
            "tun.auto-route",
            "未配置",
            "建议显式设置 tun.auto-route: true",
        ),
    });
    items.push(match auto_detect_interface {
        Some(true) => pass("tun.auto-detect-interface", "已开启"),
        Some(false) => warn(
            "tun.auto-detect-interface",
            "已关闭",
            "建议开启 auto-detect-interface，减少多网卡误判",
        ),
        None => warn(
            "tun.auto-detect-interface",
            "未配置",
            "建议显式设置 tun.auto-detect-interface: true",
        ),
    });
    items.push(match (auto_redirect, auto_route) {
        (true, Some(true)) => pass("tun.auto-redirect", "已开启且依赖满足(auto-route=true)"),
        (true, _) => fail(
            "tun.auto-redirect",
            "已开启但 auto-route 未开启",
            "请先开启 tun.auto-route",
        ),
        (false, _) => warn(
            "tun.auto-redirect",
            "已关闭",
            "Linux 下建议开启以增强 TCP 转发性能",
        ),
    });
    items.push(match strict_route {
        Some(true) => pass("tun.strict-route", "已开启"),
        Some(false) => warn("tun.strict-route", "已关闭", "建议按场景评估后开启"),
        None => warn(
            "tun.strict-route",
            "未配置",
            "建议显式配置 strict-route，避免行为不确定",
        ),
    });
    items.push(match tun_stack {
        Some(v) if v == "mixed" => pass("tun.stack", "当前为 mixed（推荐）"),
        Some(v) if v == "gvisor" => warn("tun.stack", "当前为 gvisor", "文档建议优先使用 mixed"),
        Some(v) if v == "system" => warn(
            "tun.stack",
            "当前为 system",
            "请确认当前内核网络栈与路由策略是否匹配",
        ),
        Some(v) => warn("tun.stack", &format!("当前为 {v}"), "请确认该值受支持"),
        None => warn("tun.stack", "未配置", "建议显式设置 tun.stack: mixed"),
    });
    items.push(match dns_enable {
        Some(true) => pass("dns.enable", "已开启"),
        Some(false) => warn("dns.enable", "已关闭", "tun 场景建议开启 dns，避免解析绕过"),
        None => warn("dns.enable", "未配置", "建议显式设置 dns.enable: true"),
    });
    items.push(match dns_mode {
        Some(v) if v == "fake-ip" => pass("dns.enhanced-mode", "当前为 fake-ip（推荐）"),
        Some(v) => warn(
            "dns.enhanced-mode",
            &format!("当前为 {v}"),
            "tun 场景建议使用 fake-ip",
        ),
        None => warn(
            "dns.enhanced-mode",
            "未配置",
            "建议显式设置 dns.enhanced-mode: fake-ip",
        ),
    });

    (items, tun_enable, auto_redirect)
}

fn select_rule_backend() -> Result<RuleBackend> {
    if command_exists("nft") {
        return Ok(RuleBackend::Nft);
    }
    if command_exists("iptables") {
        return Ok(RuleBackend::Iptables);
    }
    bail!("未检测到 nft/iptables，无法下发 tun 数据面规则")
}

fn detect_active_rule_backend() -> RuleBackend {
    if nft_rules_active() {
        return RuleBackend::Nft;
    }
    if iptables_rules_active() {
        return RuleBackend::Iptables;
    }
    RuleBackend::None
}

fn apply_dataplane_rules(backend: RuleBackend, redir_port: u16) -> Result<RuleBackend> {
    match backend {
        RuleBackend::Nft => match apply_nft_rules(redir_port) {
            Ok(()) => Ok(RuleBackend::Nft),
            Err(nft_err) => {
                if command_exists("iptables") {
                    // nft 失败时自动回退 iptables，提升跨环境可用性。
                    apply_iptables_rules(redir_port).with_context(|| {
                        format!("nft 下发失败后回退 iptables 仍失败（nft 错误: {nft_err}）")
                    })?;
                    Ok(RuleBackend::Iptables)
                } else {
                    Err(nft_err)
                }
            }
        },
        RuleBackend::Iptables => {
            apply_iptables_rules(redir_port)?;
            Ok(RuleBackend::Iptables)
        }
        RuleBackend::None => Ok(RuleBackend::None),
    }
}

fn cleanup_dataplane_rules(backend: RuleBackend) -> Result<()> {
    match backend {
        RuleBackend::Nft => cleanup_nft_rules(),
        RuleBackend::Iptables => cleanup_iptables_rules(),
        RuleBackend::None => Ok(()),
    }
}

fn cleanup_dataplane_rules_all() -> Result<()> {
    let mut errors = Vec::new();
    if let Err(err) = cleanup_nft_rules() {
        errors.push(format!("nft: {err}"));
    }
    if let Err(err) = cleanup_iptables_rules() {
        errors.push(format!("iptables: {err}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("清理规则失败: {}", errors.join("; "))
    }
}

fn cleanup_dataplane_rules_all_best_effort() {
    if let Err(err) = cleanup_dataplane_rules_all() {
        if !is_json_mode() {
            eprintln!("警告: 清理历史规则失败: {}", err);
        }
    }
}

fn apply_nft_rules(redir_port: u16) -> Result<()> {
    if !command_exists("nft") {
        bail!("未检测到 nft 命令");
    }
    let _ = run_cmd("nft", &["delete", "table", "inet", NFT_TABLE_NAME]);

    let script = format!(
        "table inet {table} {{
  chain prerouting {{
    type nat hook prerouting priority dstnat; policy accept;
    ip daddr {{ 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 198.18.0.0/15, 224.0.0.0/4, 240.0.0.0/4 }} return
    ip6 daddr {{ ::1/128, fc00::/7, fe80::/10, ff00::/8 }} return
    tcp dport {{ 7890, 7891, 9090, {port} }} return
    meta l4proto tcp redirect to :{port}
  }}
  chain output {{
    type nat hook output priority -100; policy accept;
    meta skuid 0 return
    ip daddr {{ 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 198.18.0.0/15, 224.0.0.0/4, 240.0.0.0/4 }} return
    ip6 daddr {{ ::1/128, fc00::/7, fe80::/10, ff00::/8 }} return
    tcp dport {{ 7890, 7891, 9090, {port} }} return
    meta l4proto tcp redirect to :{port}
  }}
}}",
        table = NFT_TABLE_NAME,
        port = redir_port
    );
    run_cmd_with_stdin("nft", &["-f", "-"], &script)?;
    if !nft_rules_active() {
        bail!("nft 规则下发后校验失败");
    }
    Ok(())
}

fn cleanup_nft_rules() -> Result<()> {
    if !command_exists("nft") {
        return Ok(());
    }
    if nft_rules_active() {
        run_cmd("nft", &["delete", "table", "inet", NFT_TABLE_NAME])?;
    }
    Ok(())
}

fn apply_iptables_rules(redir_port: u16) -> Result<()> {
    if !command_exists("iptables") {
        bail!("未检测到 iptables 命令");
    }
    configure_iptables_binary("iptables", redir_port, false)?;
    configure_iptables_binary("ip6tables", redir_port, true)?;
    if !iptables_rules_active() {
        bail!("iptables 规则下发后校验失败");
    }
    Ok(())
}

fn cleanup_iptables_rules() -> Result<()> {
    cleanup_iptables_binary("iptables", false)?;
    cleanup_iptables_binary("ip6tables", true)?;
    Ok(())
}

fn configure_iptables_binary(binary: &str, redir_port: u16, optional: bool) -> Result<()> {
    if !command_exists(binary) {
        if optional {
            return Ok(());
        }
        bail!("未检测到 {} 命令", binary);
    }

    let _ = run_cmd(binary, &["-t", "nat", "-N", IPT_CHAIN_NAME]);
    run_cmd(binary, &["-t", "nat", "-F", IPT_CHAIN_NAME])?;

    let private_ipv4 = [
        "127.0.0.0/8",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "198.18.0.0/15",
        "224.0.0.0/4",
        "240.0.0.0/4",
    ];
    let private_ipv6 = ["::1/128", "fc00::/7", "fe80::/10", "ff00::/8"];

    if binary == "ip6tables" {
        for cidr in private_ipv6 {
            run_cmd(
                binary,
                &[
                    "-t",
                    "nat",
                    "-A",
                    IPT_CHAIN_NAME,
                    "-d",
                    cidr,
                    "-j",
                    "RETURN",
                ],
            )?;
        }
    } else {
        for cidr in private_ipv4 {
            run_cmd(
                binary,
                &[
                    "-t",
                    "nat",
                    "-A",
                    IPT_CHAIN_NAME,
                    "-d",
                    cidr,
                    "-j",
                    "RETURN",
                ],
            )?;
        }
    }

    let mut bypass_ports = vec![7890_u16, 7891_u16, 9090_u16, redir_port];
    bypass_ports.sort_unstable();
    bypass_ports.dedup();
    for bypass_port in bypass_ports {
        let bypass_port_s = bypass_port.to_string();
        run_cmd(
            binary,
            &[
                "-t",
                "nat",
                "-A",
                IPT_CHAIN_NAME,
                "-p",
                "tcp",
                "--dport",
                &bypass_port_s,
                "-j",
                "RETURN",
            ],
        )?;
    }

    let port = redir_port.to_string();
    run_cmd(
        binary,
        &[
            "-t",
            "nat",
            "-A",
            IPT_CHAIN_NAME,
            "-p",
            "tcp",
            "-j",
            "REDIRECT",
            "--to-ports",
            &port,
        ],
    )?;

    ensure_iptables_jump(binary, "PREROUTING", false)?;
    ensure_iptables_jump(binary, "OUTPUT", true)?;
    Ok(())
}

fn ensure_iptables_jump(binary: &str, hook: &str, non_root_only: bool) -> Result<()> {
    if non_root_only {
        if !check_cmd_success(
            binary,
            &[
                "-t",
                "nat",
                "-C",
                hook,
                "-p",
                "tcp",
                "-m",
                "owner",
                "!",
                "--uid-owner",
                "0",
                "-j",
                IPT_CHAIN_NAME,
            ],
        ) {
            run_cmd(
                binary,
                &[
                    "-t",
                    "nat",
                    "-A",
                    hook,
                    "-p",
                    "tcp",
                    "-m",
                    "owner",
                    "!",
                    "--uid-owner",
                    "0",
                    "-j",
                    IPT_CHAIN_NAME,
                ],
            )?;
        }
        return Ok(());
    }

    if !check_cmd_success(
        binary,
        &["-t", "nat", "-C", hook, "-p", "tcp", "-j", IPT_CHAIN_NAME],
    ) {
        run_cmd(
            binary,
            &["-t", "nat", "-A", hook, "-p", "tcp", "-j", IPT_CHAIN_NAME],
        )?;
    }
    Ok(())
}

fn cleanup_iptables_binary(binary: &str, optional: bool) -> Result<()> {
    if !command_exists(binary) {
        if optional {
            return Ok(());
        }
        bail!("未检测到 {} 命令", binary);
    }

    cleanup_iptables_jump(binary, "PREROUTING", false)?;
    cleanup_iptables_jump(binary, "OUTPUT", true)?;
    cleanup_iptables_jump(binary, "OUTPUT", false)?;

    let _ = run_cmd(binary, &["-t", "nat", "-F", IPT_CHAIN_NAME]);
    let _ = run_cmd(binary, &["-t", "nat", "-X", IPT_CHAIN_NAME]);
    Ok(())
}

fn cleanup_iptables_jump(binary: &str, hook: &str, non_root_only: bool) -> Result<()> {
    for _ in 0..8 {
        let exists = if non_root_only {
            check_cmd_success(
                binary,
                &[
                    "-t",
                    "nat",
                    "-C",
                    hook,
                    "-p",
                    "tcp",
                    "-m",
                    "owner",
                    "!",
                    "--uid-owner",
                    "0",
                    "-j",
                    IPT_CHAIN_NAME,
                ],
            )
        } else {
            check_cmd_success(
                binary,
                &["-t", "nat", "-C", hook, "-p", "tcp", "-j", IPT_CHAIN_NAME],
            )
        };
        if !exists {
            break;
        }

        if non_root_only {
            run_cmd(
                binary,
                &[
                    "-t",
                    "nat",
                    "-D",
                    hook,
                    "-p",
                    "tcp",
                    "-m",
                    "owner",
                    "!",
                    "--uid-owner",
                    "0",
                    "-j",
                    IPT_CHAIN_NAME,
                ],
            )?;
        } else {
            run_cmd(
                binary,
                &["-t", "nat", "-D", hook, "-p", "tcp", "-j", IPT_CHAIN_NAME],
            )?;
        }
    }
    Ok(())
}

fn nft_rules_active() -> bool {
    command_exists("nft") && check_cmd_success("nft", &["list", "table", "inet", NFT_TABLE_NAME])
}

fn iptables_rules_active() -> bool {
    let ipv4 = command_exists("iptables")
        && (check_cmd_success(
            "iptables",
            &[
                "-t",
                "nat",
                "-C",
                "PREROUTING",
                "-p",
                "tcp",
                "-j",
                IPT_CHAIN_NAME,
            ],
        ) || check_cmd_success(
            "iptables",
            &[
                "-t",
                "nat",
                "-C",
                "OUTPUT",
                "-p",
                "tcp",
                "-m",
                "owner",
                "!",
                "--uid-owner",
                "0",
                "-j",
                IPT_CHAIN_NAME,
            ],
        ));
    let ipv6 = command_exists("ip6tables")
        && (check_cmd_success(
            "ip6tables",
            &[
                "-t",
                "nat",
                "-C",
                "PREROUTING",
                "-p",
                "tcp",
                "-j",
                IPT_CHAIN_NAME,
            ],
        ) || check_cmd_success(
            "ip6tables",
            &[
                "-t",
                "nat",
                "-C",
                "OUTPUT",
                "-p",
                "tcp",
                "-m",
                "owner",
                "!",
                "--uid-owner",
                "0",
                "-j",
                IPT_CHAIN_NAME,
            ],
        ));
    ipv4 || ipv6
}

fn read_cap_eff() -> Result<u64> {
    let content = fs::read_to_string("/proc/self/status").context("读取 /proc/self/status 失败")?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("CapEff:") {
            return u64::from_str_radix(rest.trim(), 16).context("解析 CapEff 失败");
        }
    }
    bail!("未找到 CapEff 字段")
}

fn ensure_tun_privileges() -> Result<()> {
    let is_root = is_root_user().unwrap_or(false);
    let has_admin = has_capability_bit(CAP_NET_ADMIN_BIT).unwrap_or(false);
    let has_raw = has_capability_bit(CAP_NET_RAW_BIT).unwrap_or(false);
    if is_root || (has_admin && has_raw) {
        return Ok(());
    }
    bail!(
        "当前权限不足：需要 root 或 CAP_NET_ADMIN + CAP_NET_RAW。请使用 sudo 执行，例如 `sudo clash tun on`"
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TunAction {
    Doctor,
    On,
    Off,
}

impl TunAction {
    fn as_cli_str(self) -> &'static str {
        match self {
            TunAction::Doctor => "doctor",
            TunAction::On => "on",
            TunAction::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrivilegeCheck {
    Ok,
    Delegated,
}

fn ensure_tun_privileges_or_delegate(action: TunAction, args: &TunApplyArgs) -> Result<PrivilegeCheck> {
    if ensure_tun_privileges().is_ok() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !should_auto_delegate_to_sudo() {
        ensure_tun_privileges()?;
        return Ok(PrivilegeCheck::Ok);
    }

    if !is_json_mode() {
        println!(
            "检测到权限不足，正在请求 sudo 授权继续执行 `clash tun {}` ...",
            action.as_cli_str()
        );
    }

    let status = run_tun_apply_with_sudo(action, args).context("调用 sudo 执行 tun 命令失败")?;
    if status.success() {
        return Ok(PrivilegeCheck::Delegated);
    }

    bail!(
        "sudo 授权未通过或命令执行失败，请手动执行: sudo clash tun {}",
        action.as_cli_str()
    );
}

fn ensure_tun_doctor_privileges_or_delegate() -> Result<PrivilegeCheck> {
    if ensure_tun_privileges().is_ok() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !should_auto_delegate_to_sudo() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !is_json_mode() {
        println!("检测到权限不足，正在请求 sudo 授权继续执行 `clash tun doctor` ...");
    }

    let status = run_tun_doctor_with_sudo().context("调用 sudo 执行 tun doctor 失败")?;
    if status.success() {
        return Ok(PrivilegeCheck::Delegated);
    }
    bail!("sudo 授权未通过或命令执行失败，请手动执行: sudo clash tun doctor");
}

fn should_auto_delegate_to_sudo() -> bool {
    if is_json_mode() {
        return false;
    }
    if env::var_os("CLASH_CLI_NO_AUTO_SUDO").is_some() {
        return false;
    }
    if env::var("CLASH_CLI_SUDO_REEXEC").ok().as_deref() == Some("1") {
        return false;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    command_exists("sudo")
}

fn build_sudo_reexec_command() -> Result<Command> {
    let exe = std::env::current_exe().context("获取当前可执行文件路径失败")?;
    let mut cmd = Command::new("sudo");
    cmd.arg("env");
    cmd.arg("CLASH_CLI_SUDO_REEXEC=1");
    if let Some(home) = env::var_os("CLASH_CLI_HOME") {
        cmd.arg(format!("CLASH_CLI_HOME={}", home.to_string_lossy()));
    }
    cmd.arg(exe);
    if is_json_mode() {
        cmd.arg("--json");
    }
    Ok(cmd)
}

fn run_tun_apply_with_sudo(action: TunAction, args: &TunApplyArgs) -> Result<std::process::ExitStatus> {
    let mut cmd = build_sudo_reexec_command()?;
    cmd.arg("tun");
    cmd.arg(action.as_cli_str());
    cmd.arg("--name");
    cmd.arg(&args.name);
    if args.user {
        cmd.arg("--user");
    }
    if args.no_restart {
        cmd.arg("--no-restart");
    }
    let status = cmd.status().context("启动 sudo 失败")?;
    Ok(status)
}

fn run_tun_doctor_with_sudo() -> Result<std::process::ExitStatus> {
    let mut cmd = build_sudo_reexec_command()?;
    cmd.arg("tun").arg(TunAction::Doctor.as_cli_str());
    let status = cmd.status().context("启动 sudo 失败")?;
    Ok(status)
}

fn has_capability_bit(bit: u32) -> Result<bool> {
    let mask = read_cap_eff()?;
    Ok((mask & (1u64 << bit)) != 0)
}

fn is_root_user() -> Result<bool> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("执行 `id -u` 失败")?;
    if !output.status.success() {
        bail!("`id -u` 返回非成功状态: {}", output.status);
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(uid == "0")
}

fn key_value<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    root.as_mapping()
        .and_then(|m| m.get(Value::String(key.to_string())))
}

fn bool_field(root: Option<&Value>, key: &str) -> Option<bool> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| v.as_bool())
    })
}

fn string_field(root: Option<&Value>, key: &str) -> Option<String> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
    })
}

fn u16_field(root: Option<&Value>, key: &str) -> Option<u16> {
    root.and_then(|v| {
        v.as_mapping()
            .and_then(|m| m.get(Value::String(key.to_string())))
            .and_then(|v| {
                if let Some(i) = v.as_i64() {
                    return u16::try_from(i).ok();
                }
                if let Some(s) = v.as_str() {
                    return s.parse::<u16>().ok();
                }
                None
            })
    })
}

fn load_or_init_config(path: &Path) -> Result<Value> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        fs::write(path, "{}\n")
            .with_context(|| format!("初始化配置文件失败: {}", path.display()))?;
    }
    load_existing_config(path)
}

fn load_existing_config(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?;
    let root: Value = serde_yaml::from_str(&content)
        .with_context(|| format!("解析 YAML 失败: {}", path.display()))?;
    if root.is_mapping() {
        Ok(root)
    } else {
        Ok(Value::Mapping(Mapping::new()))
    }
}

fn save_config(path: &Path, root: &Value) -> Result<()> {
    let text = serde_yaml::to_string(root).context("序列化 YAML 失败")?;
    fs::write(path, text).with_context(|| format!("写入配置失败: {}", path.display()))
}

fn set_bool_field(root: &mut Value, path_keys: &[&str], key: &str, value: bool) {
    ensure_mapping_path(root, path_keys).insert(Value::String(key.to_string()), Value::Bool(value));
}

fn set_default_bool_field(root: &mut Value, path_keys: &[&str], key: &str, value: bool) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::Bool(value));
    }
}

fn set_default_string_field(root: &mut Value, path_keys: &[&str], key: &str, value: &str) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::String(value.to_string()));
    }
}

fn set_default_u16_field(root: &mut Value, path_keys: &[&str], key: &str, value: u16) {
    let map = ensure_mapping_path(root, path_keys);
    let key_v = Value::String(key.to_string());
    if !map.contains_key(&key_v) {
        map.insert(key_v, Value::Number(Number::from(value as i64)));
    }
}

fn ensure_mapping_path<'a>(root: &'a mut Value, path_keys: &[&str]) -> &'a mut Mapping {
    if !root.is_mapping() {
        *root = Value::Mapping(Mapping::new());
    }
    let mut cursor = root;
    for key in path_keys {
        let map = cursor.as_mapping_mut().expect("mapping");
        let key_v = Value::String((*key).to_string());
        if !map.contains_key(&key_v) {
            map.insert(key_v.clone(), Value::Mapping(Mapping::new()));
        }
        let child = map.get_mut(&key_v).expect("child");
        if !child.is_mapping() {
            *child = Value::Mapping(Mapping::new());
        }
        cursor = child;
    }
    cursor.as_mapping_mut().expect("mapping")
}

fn command_exists(binary: &str) -> bool {
    check_cmd_success(binary, &["--version"]) || check_cmd_success(binary, &["-V"])
}

fn check_cmd_success(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
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

fn run_cmd_with_stdin(program: &str, args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("启动命令失败: {} {:?}", program, args))?;

    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(input.as_bytes())
            .context("写入命令 stdin 失败")?;
    }
    let output = child.wait_with_output().context("等待命令退出失败")?;
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

fn write_tun_state(path: &Path, state: TunState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    fs::write(path, state.to_text())
        .with_context(|| format!("写入 tun 状态失败: {}", path.display()))
}

fn read_tun_state(path: &Path) -> Result<Option<TunState>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 tun 状态失败: {}", path.display()))?;
    Ok(Some(TunState::from_text(&content)?))
}

fn normalize_unit_name(name: &str) -> String {
    if name.ends_with(".service") {
        name.to_string()
    } else {
        format!("{name}.service")
    }
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
    if !command_exists("systemctl") {
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

fn pass(name: &'static str, message: &str) -> CheckItem {
    CheckItem {
        name,
        level: CheckLevel::Pass,
        message: message.to_string(),
        suggestion: None,
    }
}

fn warn(name: &'static str, message: &str, suggestion: &str) -> CheckItem {
    CheckItem {
        name,
        level: CheckLevel::Warn,
        message: message.to_string(),
        suggestion: Some(suggestion.to_string()),
    }
}

fn fail(name: &'static str, message: &str, suggestion: &str) -> CheckItem {
    CheckItem {
        name,
        level: CheckLevel::Fail,
        message: message.to_string(),
        suggestion: Some(suggestion.to_string()),
    }
}

fn print_checks(checks: &[CheckItem]) -> (usize, usize, usize) {
    let (pass_count, warn_count, fail_count) = summarize_checks(checks);
    for item in checks {
        let level_str = check_level_str(item.level);
        println!("[{}] {}: {}", level_str, item.name, item.message);
        if let Some(suggestion) = &item.suggestion {
            println!("        建议: {}", suggestion);
        }
    }
    (pass_count, warn_count, fail_count)
}

fn summarize_checks(checks: &[CheckItem]) -> (usize, usize, usize) {
    let mut pass_count = 0usize;
    let mut warn_count = 0usize;
    let mut fail_count = 0usize;

    for item in checks {
        match item.level {
            CheckLevel::Pass => {
                pass_count += 1;
            }
            CheckLevel::Warn => {
                warn_count += 1;
            }
            CheckLevel::Fail => {
                fail_count += 1;
            }
        }
    }

    (pass_count, warn_count, fail_count)
}

fn check_level_str(level: CheckLevel) -> &'static str {
    match level {
        CheckLevel::Pass => "PASS",
        CheckLevel::Warn => "WARN",
        CheckLevel::Fail => "FAIL",
    }
}

fn ensure_linux_host() -> Result<()> {
    if env::consts::OS != "linux" {
        bail!("当前仅支持 Linux 平台");
    }
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
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
