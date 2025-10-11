///! Logging configuration module
///! Provides structured logging configuration with multiple outputs

use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use tracing_appender::{non_blocking, rolling};
use std::io;

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
        // Create environment filter
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(&self.level));

        // Console layer with colors
        let console_layer = fmt::layer()
            .with_target(true)
            .with_level(true)
            .with_thread_ids(false)
            .with_ansi(true)
            .with_writer(io::stdout);

        // File layer if configured
        if let Some(ref path) = self.file_path {
            // Create file appender with rotation
            let file_appender = match self.rotation {
                LogRotation::Hourly => rolling::hourly(path, "horcrux.log"),
                LogRotation::Daily => rolling::daily(path, "horcrux.log"),
                LogRotation::Never => rolling::never(path, "horcrux.log"),
            };

            let (non_blocking, _guard) = non_blocking(file_appender);

            let file_layer = fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_thread_ids(true)
                .with_ansi(false)
                .json()
                .with_writer(non_blocking);

            // Initialize with both console and file layers
            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .with(file_layer)
                .init();
        } else {
            // Console only
            tracing_subscriber::registry()
                .with(env_filter)
                .with(console_layer)
                .init();
        }

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

/// Log VM operation with context
#[macro_export]
macro_rules! log_vm_operation {
    ($op:expr, $vm_id:expr) => {
        tracing::info!(
            operation = $op,
            vm_id = $vm_id,
            "VM operation"
        )
    };
    ($op:expr, $vm_id:expr, $($key:ident = $value:expr),+) => {
        tracing::info!(
            operation = $op,
            vm_id = $vm_id,
            $($key = $value),+,
            "VM operation"
        )
    };
}

/// Log API request
#[macro_export]
macro_rules! log_api_request {
    ($method:expr, $path:expr) => {
        tracing::debug!(
            method = $method,
            path = $path,
            "API request"
        )
    };
    ($method:expr, $path:expr, $user:expr) => {
        tracing::debug!(
            method = $method,
            path = $path,
            user = $user,
            "API request"
        )
    };
}

/// Log database operation
#[macro_export]
macro_rules! log_db_operation {
    ($op:expr, $table:expr) => {
        tracing::debug!(
            operation = $op,
            table = $table,
            "Database operation"
        )
    };
    ($op:expr, $table:expr, $id:expr) => {
        tracing::debug!(
            operation = $op,
            table = $table,
            record_id = $id,
            "Database operation"
        )
    };
}

/// Log performance metric
#[macro_export]
macro_rules! log_performance {
    ($operation:expr, $duration_ms:expr) => {
        tracing::info!(
            operation = $operation,
            duration_ms = $duration_ms,
            "Performance metric"
        )
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
