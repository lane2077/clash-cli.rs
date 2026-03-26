use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use serde_yaml::{Mapping, Value};

use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;
use crate::utils::command_exists;

use super::config::{bool_field, key_value, load_or_init_config, string_field};
use super::detect::detect_bridge_interfaces;
use super::privilege::{
    CAP_NET_ADMIN_BIT, CAP_NET_RAW_BIT, PrivilegeCheck, ensure_tun_doctor_privileges_or_delegate,
    read_cap_eff,
};

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

pub(super) fn cmd_doctor() -> Result<()> {
    super::ensure_linux_host()?;
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
        checks.push(pass(
            "数据面规则",
            "auto-redirect=true，由 mihomo 自行管理数据面规则",
        ));
    }

    // 检测 Docker 桥接接口与 tun 配置排除情况
    let bridge_ifaces = detect_bridge_interfaces();
    if !bridge_ifaces.is_empty() {
        let tun = key_value(
            &load_or_init_config(&paths.runtime_config_file)
                .unwrap_or(Value::Mapping(Mapping::new())),
            "tun",
        )
        .cloned();
        let has_include = tun
            .as_ref()
            .and_then(|t| t.as_mapping())
            .and_then(|m| m.get(Value::String("include-interface".to_string())))
            .and_then(|v| v.as_sequence())
            .map_or(false, |s| !s.is_empty());
        let has_exclude = tun
            .as_ref()
            .and_then(|t| t.as_mapping())
            .and_then(|m| m.get(Value::String("exclude-interface".to_string())))
            .and_then(|v| v.as_sequence())
            .map_or(false, |s| !s.is_empty());
        if has_include || has_exclude {
            checks.push(pass(
                "Docker 桥接隔离",
                &format!(
                    "检测到 {} 个桥接接口 ({})，tun 配置已包含接口过滤",
                    bridge_ifaces.len(),
                    bridge_ifaces.join(", ")
                ),
            ));
        } else {
            checks.push(warn(
                "Docker 桥接隔离",
                &format!(
                    "检测到 {} 个桥接接口 ({})，但 tun 未配置 include/exclude-interface",
                    bridge_ifaces.len(),
                    bridge_ifaces.join(", ")
                ),
                "建议执行 `clash tun on` 自动检测并配置接口白名单",
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

    let root = match super::config::load_existing_config(config_path) {
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
