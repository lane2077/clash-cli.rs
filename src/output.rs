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

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("序列化 JSON 失败")?;
    println!("{}", text);
    Ok(())
}
