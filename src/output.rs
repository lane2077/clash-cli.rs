use std::cell::{Cell, RefCell};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::machine::ActionSemantics;

thread_local! {
    static MACHINE_MODE: Cell<bool> = const { Cell::new(false) };
    static MACHINE_ACTION: RefCell<String> = RefCell::new("unknown".to_string());
    static MACHINE_MUTATING: Cell<bool> = const { Cell::new(false) };
}

pub fn set_machine_mode(enabled: bool) {
    MACHINE_MODE.with(|value| value.set(enabled));
}

pub fn is_machine_mode() -> bool {
    MACHINE_MODE.with(Cell::get)
}

pub fn set_machine_context(action: impl Into<String>, semantics: ActionSemantics) {
    MACHINE_ACTION.with(|value| *value.borrow_mut() = action.into());
    MACHINE_MUTATING.with(|value| value.set(semantics.mutating));
}

fn machine_action() -> String {
    MACHINE_ACTION.with(|value| value.borrow().clone())
}

fn machine_semantics() -> ActionSemantics {
    if MACHINE_MUTATING.with(Cell::get) {
        ActionSemantics::WRITE
    } else {
        ActionSemantics::READ
    }
}

pub fn print_machine<T: Serialize>(value: &T) -> Result<()> {
    let payload = serde_json::to_value(value).context("序列化机器输出失败")?;
    let value = crate::machine::success_envelope(&payload, &machine_action(), machine_semantics());
    let text = serde_json::to_string_pretty(&value).context("序列化机器输出失败")?;
    println!("{text}");
    Ok(())
}

pub fn print_machine_error(err: &anyhow::Error) -> Result<()> {
    let value = crate::machine::error_envelope(err, &machine_action());
    let text = serde_json::to_string_pretty(&value).context("序列化机器错误失败")?;
    println!("{text}");
    Ok(())
}
