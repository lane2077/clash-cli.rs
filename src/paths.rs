use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};

use crate::utils;

static HOME_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// 设置全局 home 目录覆盖（安全，无需 unsafe env::set_var）。
/// 仅在 setup init/unify 等需要强制指定 home 的场景调用。
pub fn set_home_override(path: PathBuf) {
    let _ = HOME_OVERRIDE.set(path);
}

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub state_file: PathBuf,
    pub env_file: PathBuf,
    pub profile_dir: PathBuf,
    pub profile_index_file: PathBuf,
    pub profile_mixin_file: PathBuf,
    pub core_dir: PathBuf,
    pub core_versions_dir: PathBuf,
    pub core_current_link: PathBuf,
    pub core_meta_file: PathBuf,
    pub runtime_dir: PathBuf,
    pub runtime_config_file: PathBuf,
    pub runtime_tun_state_file: PathBuf,
}

pub fn app_paths() -> Result<AppPaths> {
    let config_dir = if let Some(override_home) = HOME_OVERRIDE.get() {
        override_home.clone()
    } else if let Some(custom) = env::var_os("CLASH_CLI_HOME") {
        PathBuf::from(custom)
    } else if utils::is_root_user() {
        // Linux 服务场景下，root 默认统一使用系统目录，避免落到 /root/.config 造成双配置源。
        PathBuf::from("/etc/clash-cli")
    } else if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("clash-cli")
    } else {
        let home = dirs::home_dir().context("无法获取 home 目录")?;
        home.join(".config").join("clash-cli")
    };

    let core_dir = config_dir.join("core");
    let profile_dir = config_dir.join("profiles");
    Ok(AppPaths {
        state_file: config_dir.join("proxy.state"),
        env_file: config_dir.join("proxy.env"),
        profile_index_file: profile_dir.join("index.json"),
        profile_mixin_file: profile_dir.join("mixin.yaml"),
        profile_dir,
        runtime_dir: config_dir.join("runtime"),
        runtime_config_file: config_dir.join("runtime").join("config.yaml"),
        runtime_tun_state_file: config_dir.join("runtime").join("tun.state"),
        config_dir,
        core_versions_dir: core_dir.join("versions"),
        core_current_link: core_dir.join("mihomo"),
        core_meta_file: core_dir.join("current.meta"),
        core_dir,
    })
}
