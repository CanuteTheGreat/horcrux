//! Audit log rotation and archival
//!
//! Provides automatic rotation of audit logs based on:
//! - File size
//! - Time-based rotation (daily, weekly)
//! - Maximum number of archived files

use chrono::{DateTime, Datelike, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{error, info, warn};

/// Rotation strategy
#[derive(Debug, Clone)]
pub enum RotationStrategy {
    /// Rotate when file exceeds size in bytes
    Size(u64),
    /// Rotate daily at specified hour (0-23)
    Daily { hour: u32 },
    /// Rotate weekly on specified day (0=Sunday, 6=Saturday) at hour
    Weekly { day: u32, hour: u32 },
    /// Rotate when file exceeds size OR time threshold
    Combined { max_size: u64, max_age_hours: u32 },
}

/// Configuration for audit log rotation
#[derive(Debug, Clone)]
pub struct RotationConfig {
    /// Path to the audit log file
    pub log_path: PathBuf,
    /// Directory to store archived logs
    pub archive_dir: PathBuf,
    /// Rotation strategy
    pub strategy: RotationStrategy,
    /// Maximum number of archived files to keep
    pub max_archives: usize,
    /// Compress archived files
    pub compress: bool,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            log_path: PathBuf::from("/var/log/horcrux/audit.log"),
            archive_dir: PathBuf::from("/var/log/horcrux/audit-archive"),
            strategy: RotationStrategy::Combined {
                max_size: 100 * 1024 * 1024, // 100 MB
                max_age_hours: 24,
            },
            max_archives: 30, // Keep 30 days of archives
            compress: true,
        }
    }
}

/// Audit log rotator
pub struct AuditRotator {
    config: RotationConfig,
    last_rotation: DateTime<Utc>,
}

impl AuditRotator {
    /// Create a new audit rotator
    pub fn new(config: RotationConfig) -> Self {
        Self {
            config,
            last_rotation: Utc::now(),
        }
    }

    /// Check if rotation is needed based on the strategy
    pub async fn needs_rotation(&self) -> bool {
        match &self.config.strategy {
            RotationStrategy::Size(max_size) => self.check_size_rotation(*max_size).await,
            RotationStrategy::Daily { hour } => self.check_daily_rotation(*hour),
            RotationStrategy::Weekly { day, hour } => self.check_weekly_rotation(*day, *hour),
            RotationStrategy::Combined {
                max_size,
                max_age_hours,
            } => {
                self.check_size_rotation(*max_size).await || self.check_age_rotation(*max_age_hours)
            }
        }
    }

    /// Check if rotation is needed based on file size
    async fn check_size_rotation(&self, max_size: u64) -> bool {
        match fs::metadata(&self.config.log_path).await {
            Ok(metadata) => metadata.len() >= max_size,
            Err(_) => false,
        }
    }

    /// Check if rotation is needed based on daily schedule
    fn check_daily_rotation(&self, _hour: u32) -> bool {
        let now = Utc::now();
        let last_date = self.last_rotation.date_naive();
        let current_date = now.date_naive();

        current_date > last_date
    }

    /// Check if rotation is needed based on weekly schedule
    fn check_weekly_rotation(&self, target_day: u32, _hour: u32) -> bool {
        let now = Utc::now();
        let current_weekday = now.weekday().num_days_from_sunday();

        if current_weekday != target_day {
            return false;
        }

        // Check if we already rotated today
        let last_date = self.last_rotation.date_naive();
        let current_date = now.date_naive();

        current_date > last_date
    }

    /// Check if rotation is needed based on age
    fn check_age_rotation(&self, max_age_hours: u32) -> bool {
        let now = Utc::now();
        let age = now - self.last_rotation;
        age.num_hours() >= max_age_hours as i64
    }

    /// Perform log rotation
    pub async fn rotate(&mut self) -> Result<PathBuf, RotationError> {
        // Ensure archive directory exists
        fs::create_dir_all(&self.config.archive_dir)
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to create archive dir: {}", e)))?;

        // Generate archive filename with timestamp
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let archive_name = format!("audit_{}.log", timestamp);
        let archive_path = self.config.archive_dir.join(&archive_name);

        // Check if source file exists
        if !self.config.log_path.exists() {
            return Err(RotationError::FileNotFound(
                self.config.log_path.to_string_lossy().to_string(),
            ));
        }

        // Move current log to archive
        fs::rename(&self.config.log_path, &archive_path)
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to move log file: {}", e)))?;

        // Create new empty log file
        fs::write(&self.config.log_path, "")
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to create new log file: {}", e)))?;

        // Compress if enabled
        let final_path = if self.config.compress {
            self.compress_archive(&archive_path).await?
        } else {
            archive_path.clone()
        };

        // Cleanup old archives
        self.cleanup_old_archives().await?;

        // Update last rotation time
        self.last_rotation = Utc::now();

        info!(
            archive = %final_path.display(),
            "Audit log rotated successfully"
        );

        Ok(final_path)
    }

    /// Compress an archive file using gzip
    ///
    /// Implemented with the `flate2` crate so compression happens entirely
    /// in-process. The previous implementation shelled out to a system
    /// `gzip` binary, which (a) made audit log rotation silently depend on
    /// an external executable being present on `PATH` -- on a minimal
    /// Gentoo install with no `app-arch/gzip` merged, rotation would quietly
    /// fall back to leaving every archive uncompressed forever, with only a
    /// `warn!` log line and no operator-facing signal -- and (b) read the
    /// original file into `_content` and then never used it, spawning a
    /// subprocess against `archive_path` instead; the in-memory read was
    /// pure dead work on every single rotation.
    async fn compress_archive(&self, archive_path: &Path) -> Result<PathBuf, RotationError> {
        let compressed_path = archive_path.with_extension("log.gz");

        // Read original file
        let content = fs::read(archive_path)
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to read archive: {}", e)))?;

        let compressed_path_for_blocking = compressed_path.clone();
        let compress_result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let out_file = std::fs::File::create(&compressed_path_for_blocking)?;
            let mut encoder = GzEncoder::new(out_file, Compression::default());
            encoder.write_all(&content)?;
            encoder.finish()?;
            Ok(())
        })
        .await
        .map_err(|e| {
            RotationError::CompressionError(format!("compression task panicked: {}", e))
        })?;

        if let Err(e) = compress_result {
            // Best-effort cleanup of any partially-written .gz file so a
            // failed compression doesn't leave a corrupt archive behind.
            let _ = std::fs::remove_file(&compressed_path);
            warn!(error = %e, "gzip compression failed, keeping uncompressed archive");
            return Ok(archive_path.to_path_buf());
        }

        // Compression succeeded -- remove the uncompressed original so we
        // don't keep both copies of every rotated archive on disk forever.
        if let Err(e) = fs::remove_file(archive_path).await {
            warn!(
                error = %e,
                "failed to remove uncompressed archive after successful gzip compression"
            );
        }

        Ok(compressed_path)
    }

    /// Remove old archives exceeding max_archives limit
    async fn cleanup_old_archives(&self) -> Result<(), RotationError> {
        let mut archives = Vec::new();

        // List all archive files
        let mut entries = fs::read_dir(&self.config.archive_dir)
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to read archive dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.is_file() {
                let metadata = fs::metadata(&path).await.map_err(|e| {
                    RotationError::IoError(format!("Failed to get metadata: {}", e))
                })?;

                if let Ok(modified) = metadata.modified() {
                    archives.push((path, modified));
                }
            }
        }

        // Sort by modification time (oldest first)
        archives.sort_by_key(|a| a.1);

        // Remove oldest archives if exceeding limit
        let to_remove = archives.len().saturating_sub(self.config.max_archives);
        for (path, _) in archives.iter().take(to_remove) {
            if let Err(e) = fs::remove_file(path).await {
                error!(path = %path.display(), error = %e, "Failed to remove old archive");
            } else {
                info!(path = %path.display(), "Removed old audit archive");
            }
        }

        Ok(())
    }

    /// Get list of archived log files
    pub async fn list_archives(&self) -> Result<Vec<ArchiveInfo>, RotationError> {
        let mut archives = Vec::new();

        if !self.config.archive_dir.exists() {
            return Ok(archives);
        }

        let mut entries = fs::read_dir(&self.config.archive_dir)
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to read archive dir: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| RotationError::IoError(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.is_file() {
                let metadata = fs::metadata(&path).await.map_err(|e| {
                    RotationError::IoError(format!("Failed to get metadata: {}", e))
                })?;

                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                let created = metadata
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);

                archives.push(ArchiveInfo {
                    path: path.clone(),
                    filename,
                    size_bytes: metadata.len(),
                    created_timestamp: created,
                    compressed: path.extension().map(|e| e == "gz").unwrap_or(false),
                });
            }
        }

        // Sort by creation time (newest first)
        archives.sort_by_key(|x| std::cmp::Reverse(x.created_timestamp));

        Ok(archives)
    }

    /// Get total size of all archives
    pub async fn get_total_archive_size(&self) -> u64 {
        self.list_archives()
            .await
            .map(|archives| archives.iter().map(|a| a.size_bytes).sum())
            .unwrap_or(0)
    }
}

/// Information about an archived log file
#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub created_timestamp: Option<i64>,
    pub compressed: bool,
}

/// Errors that can occur during rotation
#[derive(Debug, Clone)]
pub enum RotationError {
    FileNotFound(String),
    IoError(String),
    CompressionError(String),
}

impl std::fmt::Display for RotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RotationError::FileNotFound(path) => write!(f, "Log file not found: {}", path),
            RotationError::IoError(msg) => write!(f, "I/O error: {}", msg),
            RotationError::CompressionError(msg) => write!(f, "Compression error: {}", msg),
        }
    }
}

impl std::error::Error for RotationError {}

/// Background task for automatic rotation
pub async fn start_rotation_task(mut rotator: AuditRotator, check_interval_secs: u64) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(check_interval_secs));

    loop {
        interval.tick().await;

        if rotator.needs_rotation().await {
            match rotator.rotate().await {
                Ok(archive_path) => {
                    info!(
                        archive = %archive_path.display(),
                        "Automatic audit log rotation completed"
                    );
                }
                Err(e) => {
                    error!(error = %e, "Automatic audit log rotation failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_rotation_config_default() {
        let config = RotationConfig::default();
        assert_eq!(config.max_archives, 30);
        assert!(config.compress);
    }

    #[test]
    fn test_age_rotation_check() {
        let config = RotationConfig::default();
        let mut rotator = AuditRotator::new(config);

        // Just created, should not need rotation
        assert!(!rotator.check_age_rotation(24));

        // Simulate old rotation
        rotator.last_rotation = Utc::now() - Duration::hours(25);
        assert!(rotator.check_age_rotation(24));
    }

    #[tokio::test]
    async fn test_compress_archive_produces_valid_gzip_and_removes_original() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive_path = dir.path().join("audit_test.log");
        let original_content = b"line one\nline two\nline three\n".repeat(50);
        tokio::fs::write(&archive_path, &original_content)
            .await
            .expect("write original archive");

        let config = RotationConfig {
            log_path: dir.path().join("audit.log"),
            archive_dir: dir.path().to_path_buf(),
            strategy: RotationStrategy::Size(1),
            max_archives: 30,
            compress: true,
        };
        let rotator = AuditRotator::new(config);

        let compressed_path = rotator
            .compress_archive(&archive_path)
            .await
            .expect("compress_archive should succeed");

        assert_eq!(compressed_path, archive_path.with_extension("log.gz"));
        assert!(
            compressed_path.exists(),
            "compressed .gz file should exist on disk"
        );
        assert!(
            !archive_path.exists(),
            "uncompressed original should be removed after successful compression"
        );

        // Decompress in-process (no external `gzip` binary involved) and
        // confirm we get back exactly the original bytes -- this is the
        // behavior that previously depended on a system `gzip` executable
        // being present on PATH.
        let compressed_bytes = tokio::fs::read(&compressed_path)
            .await
            .expect("read compressed archive");
        let mut decoder = flate2::read::GzDecoder::new(&compressed_bytes[..]);
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut decompressed)
            .expect("gzip stream should be valid and decode cleanly");
        assert_eq!(decompressed, original_content);
    }
}
