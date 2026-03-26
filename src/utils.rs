use std::env;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

/// 检测系统中是否存在指定的可执行文件。
/// 尝试 `--version` 和 `-V` 两种方式，抑制所有输出。
pub fn command_exists(binary: &str) -> bool {
    check_cmd_success(binary, &["--version"]) || check_cmd_success(binary, &["-V"])
}

/// 执行命令并检查是否成功，抑制所有输出。
pub(crate) fn check_cmd_success(binary: &str, args: &[&str]) -> bool {
    Command::new(binary)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn is_root_user() -> bool {
    let output = Command::new("id").arg("-u").output();
    match output {
        Ok(v) if v.status.success() => String::from_utf8_lossy(&v.stdout).trim() == "0",
        _ => false,
    }
}

pub(crate) fn ensure_linux_host() -> Result<()> {
    if env::consts::OS != "linux" {
        bail!("当前仅支持 Linux 平台");
    }
    Ok(())
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}

pub(crate) fn normalize_unit_name(name: &str) -> String {
    if name.ends_with(".service") {
        name.to_string()
    } else {
        format!("{name}.service")
    }
}
