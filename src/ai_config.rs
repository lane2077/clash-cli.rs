use crate::paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AiConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: Option<String>,
    pub protocol: Option<String>,
}

pub fn get_config_path() -> anyhow::Result<PathBuf> {
    Ok(paths::app_paths()?.config_dir.join("ai-config.json"))
}

pub fn load() -> Result<AiConfig> {
    if let Ok(path) = get_config_path() {
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                return Ok(serde_json::from_str(&content).unwrap_or_default());
            }
        }
    }
    Ok(AiConfig::default())
}

pub fn save(config: &AiConfig) -> Result<()> {
    let path = get_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}
