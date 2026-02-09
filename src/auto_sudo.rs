use std::env;
use std::io::{ErrorKind, IsTerminal};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};

pub const AUTO_SUDO_ENV: &str = "CLASH_CLI_SUDO_REEXEC";
pub const AUTO_SUDO_DISABLE_ENV: &str = "CLASH_CLI_NO_AUTO_SUDO";

pub fn should_auto_delegate(json_mode: bool) -> bool {
    if json_mode {
        return false;
    }
    if env::var_os(AUTO_SUDO_DISABLE_ENV).is_some() {
        return false;
    }
    if env::var(AUTO_SUDO_ENV).ok().as_deref() == Some("1") {
        return false;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return false;
    }
    command_exists("sudo")
}

pub fn run_with_sudo<F>(json_mode: bool, mut append_args: F) -> Result<ExitStatus>
where
    F: FnMut(&mut Command) -> Result<()>,
{
    let exe = std::env::current_exe().context("获取当前可执行文件路径失败")?;
    let mut cmd = Command::new("sudo");
    cmd.arg("env");
    cmd.arg(format!("{AUTO_SUDO_ENV}=1"));
    if let Some(home) = env::var_os("CLASH_CLI_HOME") {
        cmd.arg(format!("CLASH_CLI_HOME={}", home.to_string_lossy()));
    }
    cmd.arg(exe);
    if json_mode {
        cmd.arg("--json");
    }
    append_args(&mut cmd)?;
    cmd.status().context("启动 sudo 失败")
}

pub fn is_permission_denied_error(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            if io_err.kind() == ErrorKind::PermissionDenied {
                return true;
            }
        }
    }
    let msg = err.to_string();
    msg.contains("Permission denied")
        || msg.contains("Operation not permitted")
        || msg.contains("权限不足")
        || msg.contains("权限不够")
}

pub fn command_exists(binary: &str) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
