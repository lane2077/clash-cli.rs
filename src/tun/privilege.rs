use std::fs;

use anyhow::{Context, Result, bail};

use crate::auto_sudo;
use crate::cli::TunApplyArgs;
use crate::output::is_json_mode;
use crate::utils;

pub(super) const CAP_NET_ADMIN_BIT: u32 = 12;
pub(super) const CAP_NET_RAW_BIT: u32 = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TunAction {
    Doctor,
    On,
    Off,
}

impl TunAction {
    pub(super) fn as_cli_str(self) -> &'static str {
        match self {
            TunAction::Doctor => "doctor",
            TunAction::On => "on",
            TunAction::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrivilegeCheck {
    Ok,
    Delegated,
}

pub(super) fn read_cap_eff() -> Result<u64> {
    let content = fs::read_to_string("/proc/self/status").context("读取 /proc/self/status 失败")?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("CapEff:") {
            return u64::from_str_radix(rest.trim(), 16).context("解析 CapEff 失败");
        }
    }
    bail!("未找到 CapEff 字段")
}

fn has_capability_bit(bit: u32) -> Result<bool> {
    let mask = read_cap_eff()?;
    Ok((mask & (1u64 << bit)) != 0)
}

fn ensure_tun_privileges() -> Result<()> {
    let is_root = utils::is_root_user();
    let has_admin = has_capability_bit(CAP_NET_ADMIN_BIT).unwrap_or(false);
    let has_raw = has_capability_bit(CAP_NET_RAW_BIT).unwrap_or(false);
    if is_root || (has_admin && has_raw) {
        return Ok(());
    }
    bail!(
        "当前权限不足：需要 root 或 CAP_NET_ADMIN + CAP_NET_RAW。请使用 sudo 执行，例如 `sudo clash tun on`"
    );
}

pub(super) fn ensure_tun_privileges_or_delegate(
    action: TunAction,
    args: &TunApplyArgs,
) -> Result<PrivilegeCheck> {
    if ensure_tun_privileges().is_ok() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !auto_sudo::should_auto_delegate(is_json_mode()) {
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

pub(super) fn ensure_tun_doctor_privileges_or_delegate() -> Result<PrivilegeCheck> {
    if ensure_tun_privileges().is_ok() {
        return Ok(PrivilegeCheck::Ok);
    }

    if !auto_sudo::should_auto_delegate(is_json_mode()) {
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

fn run_tun_apply_with_sudo(
    action: TunAction,
    args: &TunApplyArgs,
) -> Result<std::process::ExitStatus> {
    auto_sudo::run_with_sudo(is_json_mode(), |cmd| {
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
        Ok(())
    })
}

fn run_tun_doctor_with_sudo() -> Result<std::process::ExitStatus> {
    auto_sudo::run_with_sudo(is_json_mode(), |cmd| {
        cmd.arg("tun").arg(TunAction::Doctor.as_cli_str());
        Ok(())
    })
}
