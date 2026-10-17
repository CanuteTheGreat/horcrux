//! Parallel backup restore
//!
//! Provides faster backup recovery by restoring multiple
//! disks/volumes in parallel. Proxmox VE 9.0 feature.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{error, info};

/// Parallel restore manager
pub struct ParallelRestoreManager {
    max_parallel: usize,
}

impl ParallelRestoreManager {
    pub fn new(max_parallel: usize) -> Self {
        Self {
            max_parallel: max_parallel.max(1).min(8), // 1-8 parallel streams
        }
    }

    /// Restore backup with parallel disk recovery
    pub async fn restore_parallel(
        &self,
        backup_id: &str,
        volumes: Vec<VolumeRestore>,
    ) -> Result<RestoreResult> {
        info!(
            "Starting parallel restore of {} with {} volumes",
            backup_id,
            volumes.len()
        );

        let start_time = std::time::Instant::now();
        let semaphore = Arc::new(Semaphore::new(self.max_parallel));
        let mut join_set = JoinSet::new();

        let total_size: u64 = volumes.iter().map(|v| v.size_bytes).sum();
        let mut completed_size = Arc::new(tokio::sync::RwLock::new(0u64));

        // Spawn restore tasks for each volume
        for volume in volumes {
            let sem = semaphore.clone();
            let completed = completed_size.clone();

            join_set.spawn(async move {
                // Acquire semaphore permit. `acquire()` only errors if the
                // semaphore has been closed, which this manager never does;
                // propagate as a regular restore error instead of panicking
                // so a future refactor can't turn one bad volume into a
                // crashed restore task.
                let _permit = match sem.acquire().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        return Err(Error::System(format!(
                            "Volume '{}': restore concurrency semaphore was closed unexpectedly",
                            volume.name
                        )));
                    }
                };

                info!("Restoring volume: {}", volume.name);

                // Restore this volume: validates the backup source exists,
                // verifies its checksum (if present) before touching the
                // target, then decompresses/copies it into place.
                let result = Self::restore_volume(&volume).await;

                // Update progress
                if result.is_ok() {
                    let mut comp = completed.write().await;
                    *comp += volume.size_bytes;
                }

                result.map(|_| volume.name.clone())
            });
        }

        // Collect results
        let mut restored_volumes = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(volume_name)) => {
                    info!("Successfully restored volume: {}", volume_name);
                    restored_volumes.push(volume_name);
                }
                Ok(Err(e)) => {
                    error!("Volume restore failed: {}", e);
                    errors.push(e.to_string());
                }
                Err(e) => {
                    error!("Task join error: {}", e);
                    errors.push(format!("Task error: {}", e));
                }
            }
        }

        let elapsed = start_time.elapsed();
        let completed_total = *completed_size.read().await;

        let throughput_mbps = if elapsed.as_secs() > 0 {
            (completed_total as f64 / 1024.0 / 1024.0) / elapsed.as_secs_f64()
        } else {
            0.0
        };

        info!(
            "Parallel restore completed: {} volumes in {:.2}s ({:.2} MB/s)",
            restored_volumes.len(),
            elapsed.as_secs_f64(),
            throughput_mbps
        );

        Ok(RestoreResult {
            backup_id: backup_id.to_string(),
            restored_volumes,
            failed_volumes: errors,
            total_size_bytes: total_size,
            duration_secs: elapsed.as_secs_f64(),
            throughput_mbps,
        })
    }

    /// Restore a single volume from its on-disk backup file to its target
    /// path.
    ///
    /// `VolumeRestore::source_path` is expected to be a regular file
    /// previously produced by the backup path (see
    /// `backup::mod::BackupManager::backup_with_file_copy` /
    /// `backup_with_lvm_snapshot`, which write a `tar` stream, optionally
    /// piped through `gzip`/`lzop`/`zstd` based on file extension, to a
    /// single archive file on the configured backup storage). Volume-level
    /// callers of this manager (anything that builds a `VolumeRestore`) are
    /// expected to point `source_path` at one such archive and
    /// `target_path` at the directory the archive should be extracted into
    /// - mirroring `restore_from_compressed_backup` /
    /// `restore_from_uncompressed_backup` in `backup::mod`, just invoked
    /// per-volume instead of per whole-VM backup.
    async fn restore_volume(volume: &VolumeRestore) -> Result<()> {
        let source = Path::new(&volume.source_path);

        let metadata = tokio::fs::metadata(source).await.map_err(|e| {
            Error::System(format!(
                "Volume '{}': backup source '{}' is not accessible: {}",
                volume.name, volume.source_path, e
            ))
        })?;

        if !metadata.is_file() {
            return Err(Error::System(format!(
                "Volume '{}': backup source '{}' is not a regular file (archive expected)",
                volume.name, volume.source_path
            )));
        }

        // Verify checksum (sha256) against the archive on disk *before*
        // touching the target, so a corrupted backup never partially
        // overwrites a target volume.
        if let Some(expected) = &volume.checksum {
            let actual = Self::sha256_file(source).await?;
            if &actual != expected {
                return Err(Error::System(format!(
                    "Volume '{}': checksum mismatch for '{}' (expected {}, got {})",
                    volume.name, volume.source_path, expected, actual
                )));
            }
        }

        tokio::fs::create_dir_all(&volume.target_path)
            .await
            .map_err(|e| {
                Error::System(format!(
                    "Volume '{}': failed to create target directory '{}': {}",
                    volume.name, volume.target_path, e
                ))
            })?;

        // Pick the decompressor the same way `backup::mod` picks the
        // compressor: by archive file extension.
        let decompress_cmd = match source
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
        {
            Some(ext) if ext == "gz" || ext == "tgz" => "gunzip",
            Some(ext) if ext == "lzo" => "lzop -d",
            Some(ext) if ext == "zst" => "unzstd",
            _ => "cat",
        };

        let extract_cmd = format!(
            "{} < {} | tar -xf - -C {}",
            decompress_cmd,
            shell_escape(&volume.source_path),
            shell_escape(&volume.target_path)
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&extract_cmd)
            .output()
            .await
            .map_err(|e| {
                Error::System(format!(
                    "Volume '{}': failed to spawn restore extraction: {}",
                    volume.name, e
                ))
            })?;

        if !output.status.success() {
            return Err(Error::System(format!(
                "Volume '{}': restore extraction failed: {}",
                volume.name,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        info!(
            "Volume '{}' restored from '{}' to '{}'",
            volume.name, volume.source_path, volume.target_path
        );

        Ok(())
    }

    /// Compute the sha256 hex digest of a file by streaming it in chunks
    /// (avoids loading multi-GB volume archives fully into memory).
    async fn sha256_file(path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncReadExt;

        let mut file = tokio::fs::File::open(path).await.map_err(|e| {
            Error::System(format!(
                "Failed to open '{}' for checksum: {}",
                path.display(),
                e
            ))
        })?;

        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        loop {
            let n = file.read(&mut buf).await.map_err(|e| {
                Error::System(format!(
                    "Failed to read '{}' for checksum: {}",
                    path.display(),
                    e
                ))
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }

        Ok(hex::encode(hasher.finalize()))
    }

    /// Calculate optimal parallel stream count based on hardware
    pub fn calculate_optimal_streams(total_size_gb: u64, available_bandwidth_gbps: f64) -> usize {
        // Simple heuristic:
        // - Small backups (<100GB): 2-4 streams
        // - Medium backups (100-500GB): 4-6 streams
        // - Large backups (>500GB): 6-8 streams
        // - Limited by available bandwidth

        let size_based = if total_size_gb < 100 {
            2
        } else if total_size_gb < 500 {
            4
        } else {
            6
        };

        // Limit by bandwidth (assume 1 stream needs ~1 Gbps)
        let bandwidth_based = (available_bandwidth_gbps.ceil() as usize).min(8);

        size_based.min(bandwidth_based)
    }
}

/// Volume restore information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRestore {
    pub name: String,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: u64,
    pub checksum: Option<String>,
}

/// Restore result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub backup_id: String,
    pub restored_volumes: Vec<String>,
    pub failed_volumes: Vec<String>,
    pub total_size_bytes: u64,
    pub duration_secs: f64,
    pub throughput_mbps: f64,
}

/// Single-quote a path for safe interpolation into a `sh -c` command
/// string (same technique used elsewhere in this codebase for shelling
/// out to `tar`/`dd`/compression tools).
fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parallel_restore() {
        let manager = ParallelRestoreManager::new(4);

        // Build real tar archives so the restore path has something
        // genuine to extract, matching what `backup::mod`'s
        // `backup_with_file_copy` actually produces.
        let tmp = tempfile::tempdir().unwrap();
        let mk_volume = |name: &str, tmp: &std::path::Path| -> VolumeRestore {
            let src_dir = tmp.join(format!("{}-src", name));
            std::fs::create_dir_all(&src_dir).unwrap();
            std::fs::write(src_dir.join("data.txt"), b"hello from backup").unwrap();

            let archive_path = tmp.join(format!("{}.tar", name));
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "tar -cf {} -C {} .",
                    archive_path.display(),
                    src_dir.display()
                ))
                .status()
                .unwrap();
            assert!(status.success());

            let size_bytes = std::fs::metadata(&archive_path).unwrap().len();
            let target_dir = tmp.join(format!("{}-target", name));

            VolumeRestore {
                name: name.to_string(),
                source_path: archive_path.to_string_lossy().to_string(),
                target_path: target_dir.to_string_lossy().to_string(),
                size_bytes,
                checksum: None,
            }
        };

        let volumes = vec![
            mk_volume("disk0", tmp.path()),
            mk_volume("disk1", tmp.path()),
        ];
        let expected_total: u64 = volumes.iter().map(|v| v.size_bytes).sum();

        let result = manager
            .restore_parallel("backup-001", volumes)
            .await
            .unwrap();

        assert_eq!(result.restored_volumes.len(), 2);
        assert!(result.failed_volumes.is_empty());
        assert_eq!(result.total_size_bytes, expected_total);

        // Verify the data actually landed on disk.
        let restored = std::fs::read_to_string(tmp.path().join("disk0-target/data.txt")).unwrap();
        assert_eq!(restored, "hello from backup");
    }

    #[tokio::test]
    async fn test_restore_volume_missing_source_errors() {
        let volume = VolumeRestore {
            name: "missing".to_string(),
            source_path: "/nonexistent/path/that/should/not/exist.tar".to_string(),
            target_path: "/tmp/horcrux-test-restore-missing".to_string(),
            size_bytes: 0,
            checksum: None,
        };

        let err = ParallelRestoreManager::restore_volume(&volume)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not accessible"));
    }

    #[tokio::test]
    async fn test_restore_volume_checksum_mismatch_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("bad.tar");
        std::fs::write(&archive_path, b"not really a tar, just needs a checksum").unwrap();

        let volume = VolumeRestore {
            name: "corrupt".to_string(),
            source_path: archive_path.to_string_lossy().to_string(),
            target_path: tmp.path().join("target").to_string_lossy().to_string(),
            size_bytes: 0,
            checksum: Some("0".repeat(64)),
        };

        let err = ParallelRestoreManager::restore_volume(&volume)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
    }

    #[test]
    fn test_calculate_optimal_streams() {
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(50, 10.0),
            2
        );
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(200, 10.0),
            4
        );
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(1000, 10.0),
            6
        );
    }
}
