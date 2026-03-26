use anyhow::{Result, bail};

use crate::output::is_json_mode;
use crate::utils::{check_cmd_success, command_exists};

use super::state::RuleBackend;

const NFT_TABLE_NAME: &str = "clash_cli_tun";
const IPT_CHAIN_NAME: &str = "CLASH_CLI_TUN";

pub(super) fn detect_active_rule_backend() -> RuleBackend {
    if nft_rules_active() {
        return RuleBackend::Nft;
    }
    if iptables_rules_active() {
        return RuleBackend::Iptables;
    }
    RuleBackend::None
}

pub(super) fn cleanup_dataplane_rules(backend: RuleBackend) -> Result<()> {
    match backend {
        RuleBackend::Nft => cleanup_nft_rules(),
        RuleBackend::Iptables => cleanup_iptables_rules(),
        RuleBackend::None => Ok(()),
    }
}

pub(super) fn cleanup_dataplane_rules_all() -> Result<()> {
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

pub(super) fn cleanup_dataplane_rules_all_best_effort() {
    if let Err(err) = cleanup_dataplane_rules_all() {
        if !is_json_mode() {
            eprintln!("警告: 清理历史规则失败: {}", err);
        }
    }
}

fn cleanup_nft_rules() -> Result<()> {
    if !command_exists("nft") {
        return Ok(());
    }
    // 跳过 nft_rules_active() 以避免重复调用 command_exists("nft")
    if check_cmd_success("nft", &["list", "table", "inet", NFT_TABLE_NAME]) {
        super::run_cmd("nft", &["delete", "table", "inet", NFT_TABLE_NAME])?;
    }
    Ok(())
}

fn cleanup_iptables_rules() -> Result<()> {
    cleanup_iptables_binary("iptables", false)?;
    cleanup_iptables_binary("ip6tables", true)?;
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

    let _ = super::run_cmd(binary, &["-t", "nat", "-F", IPT_CHAIN_NAME]);
    let _ = super::run_cmd(binary, &["-t", "nat", "-X", IPT_CHAIN_NAME]);
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
            super::run_cmd(
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
            super::run_cmd(
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
