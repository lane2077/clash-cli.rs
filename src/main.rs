mod api;
mod auto_sudo;
mod cli;
mod core;
mod output;
mod paths;
mod profile;
mod proxy;
mod service;
mod setup;
mod tun;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Commands};

fn main() {
    if let Err(err) = run() {
        if output::is_json_mode() {
            let _ = output::print_json(&serde_json::json!({
                "ok": false,
                "error": err.to_string()
            }));
        } else {
            eprintln!("Error: {err}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    output::set_json_mode(cli.json);

    match cli.command {
        Commands::Proxy { command } => proxy::run(command)?,
        Commands::Core { command } => core::run(command)?,
        Commands::Service { command } => service::run(command)?,
        Commands::Tun { command } => tun::run(command)?,
        Commands::Profile { command } => profile::run(command)?,
        Commands::Api { command } => api::run(command)?,
        Commands::Setup { command } => setup::run(command)?,
    }

    Ok(())
}
