//! "Holds" on a running guest: a marker file next to a VM's first disk
//! recording that Horcrux paused or stopped the VM on purpose for an
//! operation (backup, live snapshot) and has to undo that afterwards.
//!
//! If horcrux-api dies in the middle (killed, OOM, power loss of the API
//! process only), the marker survives and the next start of the API
//! resumes the paused guest / starts the stopped one, instead of leaving a
//! customer's VM frozen with nobody knowing why.
use horcrux_common::{Result, VmHypervisor};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HoldAction {
    /// The guest was paused (QMP `stop`): resume it (`cont`).
    Resume,
    /// The guest was shut down: start it again.
    Start,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hold {
    pub action: HoldAction,
    pub reason: String,
    pub since: i64,
}

pub fn path(disk: &Path) -> PathBuf {
    disk.with_extension("hold.json")
}

/// Record a hold (written and fsynced before the guest is touched).
pub async fn set(disk: &Path, action: HoldAction, reason: &str) -> Result<()> {
    let hold = Hold {
        action,
        reason: reason.to_string(),
        since: chrono::Utc::now().timestamp(),
    };
    let json = serde_json::to_vec(&hold)
        .map_err(|e| horcrux_common::Error::System(format!("hold marker: {e}")))?;
    let p = path(disk);
    let tmp = p.with_extension("json.tmp");
    tokio::fs::write(&tmp, json).await?;
    tokio::fs::File::open(&tmp).await?.sync_all().await?;
    tokio::fs::rename(&tmp, &p).await?;
    Ok(())
}

/// Drop a hold once the guest is back in the state it was in before.
pub async fn clear(disk: &Path) {
    let _ = tokio::fs::remove_file(path(disk)).await;
}

pub fn read(disk: &Path) -> Option<Hold> {
    serde_json::from_slice(&std::fs::read(path(disk)).ok()?).ok()
}

/// Undo the holds an interrupted operation left behind (run at startup).
/// Returns the ids of the VMs that were resumed or started.
pub async fn recover(vms: &super::VmManager) -> Vec<String> {
    let mut recovered = Vec::new();
    for vm in vms.list_vms().await {
        if vm.hypervisor != VmHypervisor::Qemu {
            continue;
        }
        let Some(disk) = vm.disks.first().map(|d| PathBuf::from(&d.path)) else {
            continue;
        };
        let Some(hold) = read(&disk) else { continue };
        let running = super::qemu::QemuManager::process_running(&vm.id);
        let outcome = match (hold.action, running) {
            (HoldAction::Resume, true) => {
                let sock = super::display::DisplayRuntime::qmp_socket(&disk);
                super::snapshot::qmp_exec(&sock, "cont", None)
                    .await
                    .map(|_| ())
            }
            (HoldAction::Start, false) => {
                // A helper of the interrupted operation may still be
                // releasing the disk lock: retry briefly.
                let mut r = vms.start_vm(&vm.id).await.map(|_| ());
                for _ in 0..10 {
                    if r.is_ok() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    r = vms.start_vm(&vm.id).await.map(|_| ());
                }
                r
            }
            // Paused guest that is gone, or a stopped guest already running
            // again: nothing to undo.
            _ => Ok(()),
        };
        match outcome {
            Ok(()) => {
                tracing::warn!(
                    "VM {}: {} was interrupted ({:?} since {}); guest {:?}d",
                    vm.id,
                    hold.reason,
                    hold.action,
                    hold.since,
                    hold.action
                );
                clear(&disk).await;
                recovered.push(vm.id.clone());
            }
            Err(e) => tracing::error!(
                "VM {}: could not undo interrupted {} ({:?}): {e}",
                vm.id,
                hold.reason,
                hold.action
            ),
        }
    }
    recovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hold_round_trip() {
        let d = std::env::temp_dir().join(format!("hold-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&d).await.unwrap();
        let disk = d.join("vm1.qcow2");
        assert!(read(&disk).is_none());
        set(&disk, HoldAction::Resume, "backup").await.unwrap();
        let h = read(&disk).unwrap();
        assert_eq!(h.action, HoldAction::Resume);
        assert_eq!(h.reason, "backup");
        assert_eq!(path(&disk), d.join("vm1.hold.json"));
        clear(&disk).await;
        assert!(read(&disk).is_none());
        tokio::fs::remove_dir_all(&d).await.unwrap();
    }
}
