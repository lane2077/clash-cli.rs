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

use anyhow::{Result, anyhow};
use clap::Parser;

use crate::cli::{Cli, Commands};

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
    match Cli::try_parse_from(args.clone()) {
        Ok(cli) => run_cli(cli),
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                err.print().map_err(|e| anyhow!(e.to_string()))?;
                return Ok(());
            }

            let force_json = args.iter().any(|arg| arg == "--json")
                || std::env::var("CLASH_CLI_JSON")
                    .map(|value| !matches!(value.as_str(), "" | "0" | "false" | "False" | "FALSE"))
                    .unwrap_or(false);
            let force_text = args.iter().any(|arg| arg == "--text");
            let json = force_json || (!force_text && !output::stdout_is_tty());
            output::set_json_mode(json);
            if json {
                return Err(anyhow!(err.to_string().trim().to_string()));
            }
            err.print().map_err(|e| anyhow!(e.to_string()))?;
            Err(anyhow!("命令参数错误"))
        }
    }
}

fn run_cli(cli: Cli) -> Result<()> {
    let json = output::resolve_json_mode(
        cli.json,
        cli.text,
        output::stdout_is_tty(),
        cli.command.keeps_raw_stdout(),
    );
    output::set_json_mode(json);

    match cli.command {
        Commands::Ui { command } => ui::run(command)?,
        Commands::Mode { action, common } => api::run(cli::ApiCommand::Mode {
            action: action.unwrap_or(cli::ApiModeCommand::Get),
            common,
        })?,
        Commands::Sub { command } => profile::run(command)?,
        Commands::Proxy { command } => {
            let command = command.unwrap_or(cli::ProxyCommand::List(cli::ApiCommonArgs::default()));
            match command.into_node_api() {
                Ok(api_cmd) => api::run(api_cmd)?,
                Err(shell_cmd) => proxy::run(shell_cmd)?,
            }
        }
        Commands::System { action } => proxy::run(cli::ProxyCommand::System { action })?,
        Commands::Tun { command } => tun::run(command)?,
        Commands::Env { action } => proxy::run(cli::ProxyCommand::Env { action })?,
        Commands::Core { command } => core::run(command)?,
        Commands::Service { command } => service::run(command)?,
        Commands::Api { command } => api::run(command)?,
        Commands::Setup { command } => setup::run(command)?,
        Commands::Update { command } => update::run(command)?,
    }

    Ok(())
}

/// 按当前 `--json` 模式打印失败信息。
pub fn print_run_error(err: &anyhow::Error) {
    if output::is_json_mode() {
        let _ = output::print_json(&serde_json::json!({
            "ok": false,
            "error": err.to_string()
        }));
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
