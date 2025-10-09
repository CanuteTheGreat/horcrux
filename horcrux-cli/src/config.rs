///! CLI configuration management

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_server: String,
    pub default_output: String,
    pub token: Option<String>,
    pub username: Option<String>,
}

// Keep CliConfig as alias for backward compatibility
#[allow(dead_code)]
pub type CliConfig = Config;

impl Default for Config {
    fn default() -> Self {
        Self {
            default_server: "http://localhost:8006".to_string(),
            default_output: "table".to_string(),
            token: None,
            username: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(config_path)?;
        let config: Config = toml::from_str(&contents)?;

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self)?;
        std::fs::write(config_path, contents)?;

        Ok(())
    }

    fn config_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")?;
        Ok(PathBuf::from(home).join(".config/horcrux/cli.toml"))
    }
}
