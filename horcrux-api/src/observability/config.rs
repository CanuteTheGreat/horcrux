//! Configuration management for OpenTelemetry

use super::OtelConfig;
use std::fs;
use std::path::Path;

/// Configuration loader
pub struct ConfigLoader;

impl ConfigLoader {
    /// Load configuration from file
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<OtelConfig, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        let config: OtelConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config: {}", e))?;

        Ok(config)
    }

    /// Save configuration to file
    pub fn save_to_file<P: AsRef<Path>>(path: P, config: &OtelConfig) -> Result<(), String> {
        let content = serde_json::to_string_pretty(config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        fs::write(path, content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(())
    }

    /// Create example configuration file
    pub fn create_example_config() -> OtelConfig {
        let mut config = OtelConfig::default();
        config.enabled = true;
        config.endpoint = "http://localhost:4318".to_string();
        config.export_interval_secs = 60;

        // Example headers for authentication
        config.headers.insert(
            "Authorization".to_string(),
            "Bearer YOUR_API_KEY".to_string(),
        );

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_config_save_and_load() {
        let config = ConfigLoader::create_example_config();

        let mut temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_path_buf();

        // Save
        ConfigLoader::save_to_file(&temp_path, &config).unwrap();

        // Load
        let loaded_config = ConfigLoader::load_from_file(&temp_path).unwrap();

        assert_eq!(loaded_config.enabled, config.enabled);
        assert_eq!(loaded_config.endpoint, config.endpoint);
    }
}
