//! clash-cli 核心库。
//!
//! 二进制 `clash` 与测试 harness 共用此入口。
//! 调试时优先调用 [`harness`] 中的纯函数，避免依赖本机 systemd / launchd 状态。

mod api;
mod auto_sudo;
mod cli;
pub mod constants;
mod core;
mod http;
mod machine;
mod mixin;
mod output;
pub mod paths;
mod profile;
mod proxy;
mod service;
mod setup;
mod system_proxy;
mod tun;
mod ui;
mod update;
mod utils;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::machine::{ActionSemantics, ErrorCode, coded_error};

/// 解析当前进程参数并执行子命令（与二进制入口相同）。
pub fn run() -> Result<()> {
    run_from_args(std::env::args_os())
}

/// 供测试注入参数：首个参数应为程序名，例如 `clash`。
pub fn run_from_args<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let machine_requested = args.iter().any(|arg| arg == "--machine");

    match Cli::try_parse_from(args) {
        Ok(cli) => run_cli(cli),
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                err.print().map_err(anyhow::Error::from)?;
                return Ok(());
            }

            if machine_requested {
                output::set_machine_mode(true);
                output::set_machine_context("cli.parse", ActionSemantics::READ);
                return Err(coded_error(
                    ErrorCode::CliArgumentInvalid,
                    err.to_string().trim().to_string(),
                ));
            }
            Err(anyhow::anyhow!(err.to_string().trim().to_string()))
        }
    }
}

fn run_cli(cli: Cli) -> Result<()> {
    let action = cli.command.canonical_action();
    let semantics = cli.command.semantics();
    output::set_machine_mode(cli.machine);
    output::set_machine_context(action, semantics);

    if cli.machine && !cli.command.machine_supported() {
        return Err(coded_error(
            ErrorCode::UnsupportedMachineAction,
            format!("机器模式不支持 `{action}`；请改用可组合的原子命令，而不是交互式/持续流动作"),
        ));
    }
    if cli.machine {
        cli.command.validate_machine_inputs()?;
    }

    match cli.command {
        Commands::Contract => {
            if output::is_machine_mode() {
                output::print_machine(&machine::contract_description())?;
            } else {
                println!("Machine Contract: {}", machine::CONTRACT_VERSION);
                println!("机器调用请显式使用: clash --machine <command> ...");
                println!("完整机器契约: clash --machine contract");
            }
        }
        Commands::Ui { command } => ui::run(command)?,
        Commands::Mode { action, common } => {
            api::run_mode(action.unwrap_or(cli::ApiModeCommand::Get), common)?
        }
        Commands::Sub { command } => profile::run(command)?,
        Commands::Proxy { command } => {
            let command = command.unwrap_or(cli::ProxyCommand::List(cli::ApiCommonArgs::default()));
            match command {
                cli::ProxyCommand::List(common) => api::run_proxy_list(common)?,
                cli::ProxyCommand::Switch(args) => api::run_proxy_switch(args)?,
                other => proxy::run(other)?,
            }
        }
        Commands::System { action } => proxy::run_system(action)?,
        Commands::Tun { command } => tun::run(command)?,
        Commands::Env { action } => proxy::run_env(action)?,
        Commands::Core { command } => core::run(command)?,
        Commands::Service { command } => service::run(command)?,
        Commands::Api { command } => api::run(command)?,
        Commands::Setup { command } => setup::run(command)?,
        Commands::Update { command } => update::run(command)?,
    }

    Ok(())
}

pub fn print_run_error(err: &anyhow::Error) {
    if output::is_machine_mode() {
        let _ = output::print_machine_error(err);
    } else {
        eprintln!("Error: {err}");
    }
}

/// 测试可调用的纯函数（不依赖本机 systemd / launchd / `/dev/net/tun`）。
pub mod harness {
    pub use crate::paths::{AppPaths, app_paths};
    pub use crate::profile::{
        ApplyResult, ApplySpec, apply_subscription, merge_subscription_overlay,
        render_runtime_from_home,
    };
    pub use crate::service::{
        build_launchd_plist, build_unit_content, launchd_label, launchd_plist_config_path,
        resolve_service_binary,
    };
    pub use crate::tun::{
        DoctorCheckView, actual_tun_ok, apply_tun_policy_overlay, apply_tun_policy_overlay_for,
        docker_bridge_dataplane_note,
    };
    pub use crate::ui::{extract_web_ui_zip, ui_is_installed};
}
