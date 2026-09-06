use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::Serialize;

static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_json_mode(enabled: bool) {
    JSON_MODE.store(enabled, Ordering::Relaxed);
}

pub fn is_json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// 终端默认给人看的中文；管道/脚本（非 TTY）默认 JSON，不必记 `--json`。
/// `keep_raw_stdout` 用于 `proxy env`、`service log -f` 这类 stdout 本身就是产品。
pub fn resolve_json_mode(
    force_json: bool,
    force_text: bool,
    stdout_is_tty: bool,
    keep_raw_stdout: bool,
) -> bool {
    if force_json {
        return true;
    }
    if force_text || keep_raw_stdout {
        return false;
    }
    !stdout_is_tty
}

pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("序列化 JSON 失败")?;
    println!("{}", text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_defaults_to_text() {
        assert!(!resolve_json_mode(false, false, true, false));
    }

    #[test]
    fn pipe_defaults_to_json() {
        assert!(resolve_json_mode(false, false, false, false));
    }

    #[test]
    fn force_json_wins_on_tty() {
        assert!(resolve_json_mode(true, false, true, false));
    }

    #[test]
    fn force_text_wins_on_pipe() {
        assert!(!resolve_json_mode(false, true, false, false));
    }

    #[test]
    fn raw_stdout_commands_stay_text_on_pipe() {
        assert!(!resolve_json_mode(false, false, false, true));
    }

    #[test]
    fn force_json_overrides_raw_stdout() {
        assert!(resolve_json_mode(true, false, false, true));
    }
}
