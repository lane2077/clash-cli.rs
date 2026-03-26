use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::constants::DEFAULT_REDIR_PORT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuleBackend {
    Nft,
    Iptables,
    None,
}

impl RuleBackend {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            RuleBackend::Nft => "nft",
            RuleBackend::Iptables => "iptables",
            RuleBackend::None => "none",
        }
    }

    pub(super) fn from_str(v: &str) -> Self {
        match v {
            "nft" => RuleBackend::Nft,
            "iptables" => RuleBackend::Iptables,
            _ => RuleBackend::None,
        }
    }
}

#[derive(Debug)]
pub(super) struct TunState {
    pub(super) enabled: bool,
    pub(super) service_name: String,
    pub(super) user_service: bool,
    pub(super) backend: RuleBackend,
    pub(super) redir_port: u16,
    pub(super) rules_applied: bool,
    pub(super) updated_at: u64,
}

impl TunState {
    pub(super) fn to_text(&self) -> String {
        format!(
            "enabled={}\nservice_name={}\nuser_service={}\nbackend={}\nredir_port={}\nrules_applied={}\nupdated_at={}\n",
            self.enabled,
            self.service_name,
            self.user_service,
            self.backend.as_str(),
            self.redir_port,
            self.rules_applied,
            self.updated_at
        )
    }

    pub(super) fn from_text(content: &str) -> Result<Self> {
        let mut enabled = None;
        let mut service_name = None;
        let mut user_service = None;
        let mut backend = None;
        let mut redir_port = None;
        let mut rules_applied = None;
        let mut updated_at = None;

        for line in content.lines() {
            let mut parts = line.splitn(2, '=');
            let key = parts.next().unwrap_or_default().trim();
            let value = parts.next().unwrap_or_default().trim();
            match key {
                "enabled" => enabled = Some(value == "true"),
                "service_name" => service_name = Some(value.to_string()),
                "user_service" => user_service = Some(value == "true"),
                "backend" => backend = Some(RuleBackend::from_str(value)),
                "redir_port" => {
                    redir_port = Some(
                        value
                            .parse::<u16>()
                            .context("解析 tun.state.redir_port 失败")?,
                    )
                }
                "rules_applied" => rules_applied = Some(value == "true"),
                "updated_at" => {
                    updated_at = Some(
                        value
                            .parse::<u64>()
                            .context("解析 tun.state.updated_at 失败")?,
                    )
                }
                _ => {}
            }
        }

        Ok(Self {
            enabled: enabled.context("tun.state 缺少 enabled")?,
            service_name: service_name.context("tun.state 缺少 service_name")?,
            user_service: user_service.context("tun.state 缺少 user_service")?,
            backend: backend.unwrap_or(RuleBackend::None),
            redir_port: redir_port.unwrap_or(DEFAULT_REDIR_PORT),
            rules_applied: rules_applied.unwrap_or(false),
            updated_at: updated_at.context("tun.state 缺少 updated_at")?,
        })
    }
}

pub(super) fn write_tun_state(path: &Path, state: TunState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    fs::write(path, state.to_text())
        .with_context(|| format!("写入 tun 状态失败: {}", path.display()))
}

pub(super) fn read_tun_state(path: &Path) -> Result<Option<TunState>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 tun 状态失败: {}", path.display()))?;
    Ok(Some(TunState::from_text(&content)?))
}
