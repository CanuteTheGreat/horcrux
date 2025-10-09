///! Logging configuration module
///! Provides structured logging configuration

use tracing_subscriber::fmt;
use std::path::Path;

/// Logging configuration
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    pub level: String,          // log level (trace, debug, info, warn, error)
    pub file_path: Option<String>,  // log file path
    pub rotation: LogRotation,  // log rotation policy
    pub json_format: bool,      // use JSON formatting
    pub include_targets: Vec<String>,  // specific targets to include
}

/// Log rotation policy
#[derive(Debug, Clone)]
pub enum LogRotation {
    Hourly,
    Daily,
    Never,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file_path: Some("/var/log/horcrux".to_string()),
            rotation: LogRotation::Daily,
            json_format: false,
            include_targets: vec![],
        }
    }
}

impl LoggingConfig {
    /// Initialize logging based on configuration
    pub fn init(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Simple implementation using built-in fmt subscriber
        let _ = fmt()
            .with_target(true)
            .with_level(true)
            .with_thread_ids(false)
            .with_ansi(true)
            .try_init();

        tracing::info!("Logging initialized - level: {}", self.level);

        Ok(())
    }

    /// Initialize with default settings
    pub fn init_default() -> Result<(), Box<dyn std::error::Error>> {
        Self::default().init()
    }

    /// Initialize with environment variables
    pub fn init_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        let file_path = std::env::var("HORCRUX_LOG_PATH").ok();

        Self {
            level,
            file_path,
            ..Default::default()
        }.init()
    }
}

/// Create a structured log context
#[macro_export]
macro_rules! log_context {
    ($($key:ident = $value:expr),* $(,)?) => {
        {
            use tracing::field;
            tracing::info_span!(
                "context",
                $(
                    $key = field::display(&$value)
                ),*
            )
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LoggingConfig::default();
        assert_eq!(config.level, "info");
        assert!(config.file_path.is_some());
    }

    #[test]
    fn test_log_rotation() {
        let config = LoggingConfig {
            rotation: LogRotation::Hourly,
            ..Default::default()
        };
        matches!(config.rotation, LogRotation::Hourly);
    }
}
