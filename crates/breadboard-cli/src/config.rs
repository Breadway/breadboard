//! `~/.config/breadboard/cli.toml`: server URL + saved auth token.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    pub server: String,
    /// PAT-style token written by `breadboard auth login`.
    pub token: Option<String>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            server: "http://127.0.0.1:7280".to_string(),
            token: None,
        }
    }
}

impl CliConfig {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("breadboard").join("cli.toml"))
    }

    pub fn load() -> Result<Self> {
        match Self::path() {
            Some(p) if p.exists() => Ok(toml::from_str(&std::fs::read_to_string(p)?)?),
            _ => Ok(Self::default()),
        }
    }

    #[allow(dead_code)] // used once `breadboard auth login` lands
    pub fn save(&self) -> Result<()> {
        if let Some(p) = Self::path() {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, toml::to_string_pretty(self)?)?;
        }
        Ok(())
    }
}
