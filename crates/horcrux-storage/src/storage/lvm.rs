//! LVM storage backend
//! Provides LVM volume group and logical volume management

#![allow(dead_code)]

use super::StoragePool;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

/// LVM storage manager
pub struct LvmManager {}

impl Default for LvmManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LvmManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate an LVM volume group
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        // Check if LVM volume group exists
        let output = Command::new("vgs")
            .arg("--noheadings")
            .arg("-o")
            .arg("vg_name")
            .arg(&pool.path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to check LVM volume group: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "LVM volume group {} does not exist",
                pool.path
            )));
        }

        Ok(())
    }

    /// Create an LVM logical volume
    pub async fn create_volume(
        &self,
        vg_name: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        super::check_names(&[volume_name])?;
        info!(
            "Creating LVM volume: {}/{} ({}GB)",
            vg_name, volume_name, size_gb
        );

        let output = Command::new("lvcreate")
            .arg("-L")
            .arg(format!("{}G", size_gb))
            .arg("-n")
            .arg(volume_name)
            .arg(vg_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM volume: {}",
                stderr
            )));
        }

        Ok(format!("/dev/{}/{}", vg_name, volume_name))
    }

    /// Delete a logical volume the way the pool's options say: on a shared
    /// VG take the LV exclusively first (refused while another node uses
    /// it), zero it (`wipe_removed`) and/or discard its extents
    /// (`discard_removed`), then `lvremove`.
    pub async fn delete_volume_with(
        &self,
        vg_name: &str,
        volume_name: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume_name])?;
        let t = std::time::Duration::from_secs(opts.command_timeout_secs.into());
        let sys = |e: String| horcrux_common::Error::System(e);
        if opts.shared || opts.wipe_removed || opts.discard_removed {
            if opts.wipe_removed || opts.discard_removed {
                crate::san::tools::Need::Discard.check(&crate::san::tools::ToolStatus::probe())?;
            }
            crate::san::lvm_shared::activate_exclusive(vg_name, volume_name, t)
                .await
                .map_err(|e| {
                    if e.contains("Failed to find logical volume") {
                        horcrux_common::Error::NotFound(format!(
                            "logical volume {vg_name}/{volume_name} does not exist"
                        ))
                    } else {
                        sys(e)
                    }
                })?;
            let dev = format!("/dev/{vg_name}/{volume_name}");
            // Scrubbing a big LV takes a while: give it its own budget.
            let scrub_t = std::time::Duration::from_secs(opts.scrub_timeout_secs.into());
            crate::san::lvm_shared::scrub_before_remove(
                &dev,
                opts.wipe_removed,
                opts.discard_removed,
                scrub_t,
            )
            .await
            .map_err(sys)?;
        }
        self.delete_volume(vg_name, volume_name).await
    }

    /// Delete an LVM logical volume
    pub async fn delete_volume(&self, vg_name: &str, volume_name: &str) -> Result<()> {
        super::check_names(&[volume_name])?;
        info!("Deleting LVM volume: {}/{}", vg_name, volume_name);

        let output = Command::new("lvremove")
            .arg("-f")
            .arg(format!("{}/{}", vg_name, volume_name))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete LVM volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to delete LVM volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete LVM volume: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create an LVM snapshot (supports thick-provisioned)
    ///
    /// Proxmox VE 9 feature: snapshots on thick-provisioned LVM shared storage
    /// with snapshot chains for volume management
    pub async fn create_snapshot(
        &self,
        vg_name: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        super::check_names(&[volume_name, snapshot_name])?;
        info!(
            "Creating LVM snapshot: {}/{} -> {}",
            vg_name, volume_name, snapshot_name
        );

        // Create snapshot with same size as origin for thick-provisioned
        // This allows snapshots on FC/iSCSI SAN environments
        let output = Command::new("lvcreate")
            .arg("-s")
            .arg("-n")
            .arg(snapshot_name)
            .arg(format!("{}/{}", vg_name, volume_name))
            .arg("-L") // Thick provision snapshot
            .arg("10G") // Default snapshot size, can be customized
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create snapshot with custom size (thick-provisioned)
    pub async fn create_snapshot_sized(
        &self,
        vg_name: &str,
        volume_name: &str,
        snapshot_name: &str,
        size_gb: u64,
    ) -> Result<()> {
        super::check_names(&[volume_name, snapshot_name])?;
        info!(
            "Creating LVM snapshot: {}/{} -> {} ({}GB)",
            vg_name, volume_name, snapshot_name, size_gb
        );

        let output = Command::new("lvcreate")
            .arg("-s")
            .arg("-n")
            .arg(snapshot_name)
            .arg(format!("{}/{}", vg_name, volume_name))
            .arg("-L")
            .arg(format!("{}G", size_gb))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List snapshots for a volume (volume chains)
    pub async fn list_snapshots(&self, vg_name: &str, volume_name: &str) -> Result<Vec<String>> {
        super::check_names(&[volume_name])?;
        let output = Command::new("lvs")
            .arg("--noheadings")
            .arg("-o")
            .arg("lv_name,origin")
            .arg(vg_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list LVM snapshots: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to list LVM snapshots".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let snapshots: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 && parts[1] == volume_name {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(snapshots)
    }

    /// Get snapshot chain info (for volume chains feature)
    pub async fn get_snapshot_chain(
        &self,
        vg_name: &str,
        volume_name: &str,
    ) -> Result<SnapshotChain> {
        super::check_names(&[volume_name])?;
        let snapshots = self.list_snapshots(vg_name, volume_name).await?;

        Ok(SnapshotChain {
            origin: volume_name.to_string(),
            snapshots,
        })
    }

    /// Restore an LVM snapshot (merge)
    pub async fn restore_snapshot(
        &self,
        vg_name: &str,
        _volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        super::check_names(&[_volume_name, snapshot_name])?;
        info!("Restoring LVM snapshot: {}/{}", vg_name, snapshot_name);

        let output = Command::new("lvconvert")
            .arg("--merge")
            .arg(format!("{}/{}", vg_name, snapshot_name))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to restore LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to restore LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to restore LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    // ------------------------------------------------ snapshots as volume chain

    fn chain_t(opts: &LvmPoolOptions) -> std::time::Duration {
        std::time::Duration::from_secs(opts.command_timeout_secs.into())
    }

    fn chain_need() -> Result<()> {
        crate::san::tools::Need::VolumeChain.check(&crate::san::tools::ToolStatus::probe())
    }

    /// LV names of the volume chain of `volume` that exist in `vg`.
    async fn chain_lvs(
        &self,
        vg: &str,
        volume: &str,
        t: std::time::Duration,
    ) -> Result<Vec<String>> {
        let r = crate::san::iscsi::run("lvs", &["--noheadings", "-o", "lv_name", vg], t)
            .await
            .map_err(horcrux_common::Error::System)?;
        if !r.ok {
            return Err(horcrux_common::Error::System(format!(
                "lvs {vg}: {}",
                r.stderr.trim()
            )));
        }
        Ok(r.stdout
            .lines()
            .map(str::trim)
            .filter(|l| {
                *l == volume
                    || *l == super::lvm_chain::tmp_lv(volume)
                    || super::lvm_chain::snap_of(volume, l).is_some()
            })
            .map(str::to_string)
            .collect())
    }

    /// Activate the chain and read it (snapshots oldest first).
    pub async fn chain_info(
        &self,
        vg: &str,
        volume: &str,
        opts: &LvmPoolOptions,
    ) -> Result<super::lvm_chain::ChainView> {
        super::check_names(&[volume])?;
        Self::chain_need()?;
        let t = Self::chain_t(opts);
        let lvs = self.chain_lvs(vg, volume, t).await?;
        if !lvs.iter().any(|l| l == volume) {
            return Err(horcrux_common::Error::NotFound(format!(
                "logical volume {vg}/{volume} does not exist"
            )));
        }
        super::lvm_chain::activate(vg, &lvs, opts.shared, t)
            .await
            .map_err(horcrux_common::Error::System)?;
        super::lvm_chain::info_chain(vg, volume, t)
            .await
            .map_err(horcrux_common::Error::System)
    }

    /// New chain volume: an LV sized for a fully allocated qcow2 of
    /// `size_gb`, with the qcow2 written onto it.
    pub async fn chain_create(
        &self,
        vg: &str,
        volume: &str,
        size_gb: u64,
        opts: &LvmPoolOptions,
    ) -> Result<String> {
        super::check_names(&[volume])?;
        if super::lvm_chain::is_snap_lv(volume) {
            return Err(horcrux_common::Error::Validation(format!(
                "volume names starting with {} are reserved for snapshots",
                super::lvm_chain::SNAP_PREFIX
            )));
        }
        Self::chain_need()?;
        let t = Self::chain_t(opts);
        let vbytes = size_gb * (1 << 30);
        let lv = super::lvm_chain::measure(vbytes, t)
            .await
            .map_err(horcrux_common::Error::System)?;
        let plan = super::lvm_chain::plan_create(vg, volume, vbytes, lv);
        super::lvm_chain::run_all(&plan[..1], t)
            .await
            .map_err(horcrux_common::Error::System)?;
        if opts.shared {
            crate::san::lvm_shared::activate_exclusive(vg, volume, t)
                .await
                .map_err(horcrux_common::Error::System)?;
        }
        if let Err(e) = super::lvm_chain::run_all(&plan[1..], t).await {
            let _ = self.delete_volume(vg, volume).await;
            return Err(horcrux_common::Error::System(e));
        }
        Ok(super::lvm_chain::dev(vg, volume))
    }

    /// Refuse up front (400) when the VG cannot hold one more overlay LV
    /// of `need` bytes: a thick volume chain needs a full-size LV per
    /// snapshot.
    async fn chain_need_space(&self, vg: &str, need: u64, t: std::time::Duration) -> Result<()> {
        let r = crate::san::iscsi::run(
            "vgs",
            &[
                "--noheadings",
                "--units",
                "b",
                "--nosuffix",
                "-o",
                "vg_free",
                vg,
            ],
            t,
        )
        .await
        .map_err(horcrux_common::Error::System)?;
        match super::lvm_chain::parse_vg_free(&r.stdout) {
            Some(free) if free < need => Err(horcrux_common::Error::InvalidConfig(format!(
                "volume group {vg} has {} MiB free; this volume chain needs {} MiB for the new \
                 overlay (a snapshot on thick LVM takes the volume's full size): free space or \
                 delete an older snapshot first",
                free >> 20,
                need.div_ceil(1 << 20)
            ))),
            _ => Ok(()),
        }
    }

    /// Run `cmds`; on a failure run the undo list (best effort) and return
    /// the failure.
    async fn chain_run_or_undo(
        cmds: &[super::lvm_chain::Cmd],
        undo: &[super::lvm_chain::Cmd],
        t: std::time::Duration,
    ) -> Result<()> {
        if let Err(e) = super::lvm_chain::run_all(cmds, t).await {
            for u in undo {
                let _ = super::lvm_chain::run_all(std::slice::from_ref(u), t).await;
            }
            return Err(horcrux_common::Error::System(e));
        }
        Ok(())
    }

    /// Snapshot: freeze the current LV as `snap_<volume>_<snap>`, stack a
    /// new overlay LV on it under the volume's own name. The overlay LV is
    /// made first (under a temporary name), so a full VG or any failure
    /// leaves the volume as it was.
    pub async fn chain_snapshot(
        &self,
        vg: &str,
        volume: &str,
        snap: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume, snap])?;
        let info = self.chain_info(vg, volume, opts).await?;
        if info.snapshots.iter().any(|s| s == snap) {
            return Err(horcrux_common::Error::AlreadyExists(format!(
                "snapshot {snap} of {volume} exists"
            )));
        }
        let t = Self::chain_t(opts);
        let vbytes = info.virtual_size_bytes.ok_or_else(|| {
            horcrux_common::Error::System(format!("{vg}/{volume}: no qcow2 virtual size"))
        })?;
        let lv = super::lvm_chain::measure(vbytes, t)
            .await
            .map_err(horcrux_common::Error::System)?;
        self.chain_need_space(vg, lv, t).await?;
        let tmp = super::lvm_chain::tmp_lv(volume);
        for (i, step) in super::lvm_chain::plan_snapshot(vg, volume, snap, vbytes, lv)
            .into_iter()
            .enumerate()
        {
            Self::chain_run_or_undo(std::slice::from_ref(&step.cmd), &step.undo, t).await?;
            if i == 0 && opts.shared {
                if let Err(e) = crate::san::lvm_shared::activate_exclusive(vg, &tmp, t).await {
                    let _ = self.delete_volume(vg, &tmp).await;
                    return Err(horcrux_common::Error::System(e));
                }
            }
        }
        Ok(())
    }

    /// Roll back to the newest snapshot `snap` (older ones: delete the
    /// newer snapshots first). A fresh overlay on `snap` is built first;
    /// then the current LV is scrubbed per the pool options and removed, and
    /// the new overlay takes its name.
    pub async fn chain_rollback(
        &self,
        vg: &str,
        volume: &str,
        snap: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume, snap])?;
        let info = self.chain_info(vg, volume, opts).await?;
        match info.snapshots.last() {
            Some(s) if s == snap => {}
            _ if info.snapshots.iter().any(|s| s == snap) => {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "{snap} is not the newest snapshot of {volume}; delete the newer snapshots \
                     ({}) first",
                    info.snapshots
                        .iter()
                        .skip_while(|s| *s != snap)
                        .skip(1)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
            _ => {
                return Err(horcrux_common::Error::NotFound(format!(
                    "no snapshot {snap} of {volume}"
                )))
            }
        }
        let t = Self::chain_t(opts);
        let vbytes = info.virtual_size_bytes.unwrap_or(0);
        let lv = super::lvm_chain::measure(vbytes, t)
            .await
            .map_err(horcrux_common::Error::System)?;
        self.chain_need_space(vg, lv, t).await?;
        let tmp = super::lvm_chain::tmp_lv(volume);
        let rm_tmp = vec![vec![
            "lvremove".to_string(),
            "-y".into(),
            format!("{vg}/{tmp}"),
        ]];
        let (build, rename) = super::lvm_chain::plan_rollback_overlay(vg, volume, snap, vbytes, lv);
        Self::chain_run_or_undo(&build[..1], &[], t).await?;
        if opts.shared {
            if let Err(e) = crate::san::lvm_shared::activate_exclusive(vg, &tmp, t).await {
                let _ = self.delete_volume(vg, &tmp).await;
                return Err(horcrux_common::Error::System(e));
            }
        }
        Self::chain_run_or_undo(&build[1..], &rm_tmp, t).await?;
        if let Err(e) = self.delete_volume_with(vg, volume, opts).await {
            let _ = self.delete_volume(vg, &tmp).await;
            return Err(e);
        }
        Self::chain_run_or_undo(&[rename], &[], t).await
    }

    /// Delete snapshot `snap`: its child is rebased onto its parent (taking
    /// over `snap`'s data), then `snap`'s LV is removed (scrubbed per the
    /// pool options).
    pub async fn chain_delete_snapshot(
        &self,
        vg: &str,
        volume: &str,
        snap: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume, snap])?;
        let info = self.chain_info(vg, volume, opts).await?;
        let idx = info
            .snapshots
            .iter()
            .position(|s| s == snap)
            .ok_or_else(|| {
                horcrux_common::Error::NotFound(format!("no snapshot {snap} of {volume}"))
            })?;
        let rebase = super::lvm_chain::plan_delete_rebase(vg, volume, &info.snapshots, idx)
            .ok_or_else(|| horcrux_common::Error::System("chain changed".into()))?;
        // Copying a whole snapshot's data can take a while.
        let scrub_t = std::time::Duration::from_secs(opts.scrub_timeout_secs.into());
        super::lvm_chain::run_all(&[rebase], scrub_t)
            .await
            .map_err(horcrux_common::Error::System)?;
        self.delete_volume_with(vg, &super::lvm_chain::snap_lv(volume, snap), opts)
            .await
    }

    /// Remove a chain volume with every snapshot LV.
    pub async fn chain_delete_volume(
        &self,
        vg: &str,
        volume: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume])?;
        let t = Self::chain_t(opts);
        let lvs = self.chain_lvs(vg, volume, t).await?;
        if !lvs.iter().any(|l| l == volume) {
            return Err(horcrux_common::Error::NotFound(format!(
                "logical volume {vg}/{volume} does not exist"
            )));
        }
        // Current LV first (it references the snapshots), then the snapshots.
        self.delete_volume_with(vg, volume, opts).await?;
        for l in lvs.iter().filter(|l| *l != volume) {
            self.delete_volume_with(vg, l, opts).await?;
        }
        Ok(())
    }

    /// Check if LVM is available
    pub fn check_lvm_available() -> bool {
        std::process::Command::new("vgs")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get LVM version
    pub async fn get_lvm_version() -> Result<String> {
        let output = Command::new("lvm")
            .arg("version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get LVM version: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "LVM not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }
}

/// Snapshot chain information (Proxmox VE 9 feature)
#[derive(Debug, Clone)]
pub struct SnapshotChain {
    pub origin: String,
    pub snapshots: Vec<String>,
}

/// Per-pool LVM options (`StoragePool::lvm`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LvmPoolOptions {
    /// The VG is a shared (lvmlockd/sanlock) VG on SAN storage: LVs are
    /// activated exclusively on the node that uses them.
    #[serde(default)]
    pub shared: bool,
    /// Zero volumes before removal (`blkdiscard --zeroout`). Default false.
    #[serde(default)]
    pub wipe_removed: bool,
    /// Discard volumes' extents on removal (UNMAP/TRIM). Default false.
    #[serde(default)]
    pub discard_removed: bool,
    /// Deadline for one LVM command (default 60 s).
    #[serde(default = "default_cmd_timeout")]
    pub command_timeout_secs: u32,
    /// Deadline for wiping/discarding one volume (default 3600 s).
    #[serde(default = "default_scrub_timeout")]
    pub scrub_timeout_secs: u32,
    /// Snapshots as a volume chain (default false): volumes are qcow2 images
    /// written onto their LVs, and a snapshot freezes the LV and stacks a
    /// new qcow2 overlay LV on it (`storage::lvm_chain`). The way to get
    /// snapshots on a shared thick VG on SAN storage. Set when the pool is
    /// created; existing raw volumes are not converted.
    #[serde(default)]
    pub snapshot_as_volume_chain: bool,
}

fn default_cmd_timeout() -> u32 {
    60
}
fn default_scrub_timeout() -> u32 {
    3600
}

impl Default for LvmPoolOptions {
    fn default() -> Self {
        Self {
            shared: false,
            wipe_removed: false,
            discard_removed: false,
            command_timeout_secs: default_cmd_timeout(),
            scrub_timeout_secs: default_scrub_timeout(),
            snapshot_as_volume_chain: false,
        }
    }
}

impl LvmPoolOptions {
    pub fn validate(&self) -> Result<()> {
        if !(5..=3600).contains(&self.command_timeout_secs) {
            return Err(horcrux_common::Error::InvalidConfig(
                "lvm.command_timeout_secs must be 5..3600".into(),
            ));
        }
        if !(10..=86_400).contains(&self.scrub_timeout_secs) {
            return Err(horcrux_common::Error::InvalidConfig(
                "lvm.scrub_timeout_secs must be 10..86400".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod pool_options_tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let o: LvmPoolOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, LvmPoolOptions::default());
        assert!(!o.shared && !o.wipe_removed && !o.discard_removed);
        assert!(o.validate().is_ok());
        let bad = LvmPoolOptions {
            command_timeout_secs: 1,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }
}
