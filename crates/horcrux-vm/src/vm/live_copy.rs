//! Live (no pause) copy of the disks of a running QEMU guest through QMP
//! `blockdev-backup`; shared by backup (snapshot mode) and full clone.
use horcrux_common::Result;
use std::path::{Path, PathBuf};

/// Copy the disks of a running VM without pausing it (used by live backup and
/// live full clone): per disk a qcow2
/// target of the disk's size is attached to the VM's QEMU (`blockdev-add`)
/// and one QMP `transaction` starts a `blockdev-backup` (sync=full) job for
/// every disk at the same instant, so all copies are of one point in time
/// (guest writes after that first copy the old data out: copy-before-write).
/// The jobs are polled to completion; the targets are detached again on
/// success and on failure, and a failed or cancelled job is an error.
pub async fn live_copy_disks(sock: &Path, disks: &[(PathBuf, PathBuf)]) -> Result<()> {
    use crate::vm::snapshot::{block_device_for, qmp_exec};
    use horcrux_common::Error;
    // QEMU node names are at most 31 characters.
    let tag = format!(
        "hxbk{:08x}",
        (std::process::id() << 16) ^ chrono::Utc::now().timestamp_subsec_micros()
    );
    let mut nodes: Vec<String> = Vec::new();
    let mut actions = Vec::new();
    let mut jobs: Vec<String> = Vec::new();
    let attach = async {
        for (i, (src, dst)) in disks.iter().enumerate() {
            let device = block_device_for(sock, &src.to_string_lossy()).await?;
            let size = crate::vm::qemu::QemuManager::virtual_size(src).await?;
            crate::vm::qemu::QemuManager::create_disk_image_bytes(dst, size).await?;
            let (node, job) = (format!("{tag}-t{i}"), format!("{tag}-j{i}"));
            qmp_exec(
                sock,
                "blockdev-add",
                Some(serde_json::json!({
                    "driver": "qcow2", "node-name": node,
                    "file": {"driver": "file", "filename": dst.to_string_lossy()}
                })),
            )
            .await?;
            nodes.push(node.clone());
            actions.push(serde_json::json!({
                "type": "blockdev-backup",
                "data": {"job-id": job, "device": device, "target": node,
                         "sync": "full", "auto-dismiss": false}
            }));
            jobs.push(job);
        }
        Ok::<(), Error>(())
    }
    .await;
    let run = match attach {
        Err(e) => Err(e),
        Ok(()) => {
            async {
                qmp_exec(
                    sock,
                    "transaction",
                    Some(serde_json::json!({ "actions": actions })),
                )
                .await?;
                let mut pending = jobs.clone();
                while !pending.is_empty() {
                    let list = qmp_exec(sock, "query-jobs", None).await?;
                    let list = list.as_array().cloned().unwrap_or_default();
                    let mut still = Vec::new();
                    for job in pending {
                        let Some(j) = list.iter().find(|j| j["id"] == job.as_str()) else {
                            return Err(Error::System(format!("backup job {job} disappeared")));
                        };
                        if j["status"] == "concluded" {
                            if let Some(e) = j["error"].as_str() {
                                return Err(Error::System(format!("live backup job {job}: {e}")));
                            }
                        } else {
                            still.push(job);
                        }
                    }
                    pending = still;
                    if !pending.is_empty() {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
                Ok(())
            }
            .await
        }
    };
    if run.is_err() {
        for job in &jobs {
            let _ = qmp_exec(
                sock,
                "block-job-cancel",
                Some(serde_json::json!({"device": job, "force": true})),
            )
            .await;
        }
        for _ in 0..50 {
            let busy = qmp_exec(sock, "query-jobs", None)
                .await
                .map(|l| {
                    l.as_array().is_some_and(|a| {
                        a.iter().any(|x| {
                            jobs.iter().any(|j| x["id"] == j.as_str()) && x["status"] != "concluded"
                        })
                    })
                })
                .unwrap_or(false);
            if !busy {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    for job in &jobs {
        let _ = qmp_exec(sock, "job-dismiss", Some(serde_json::json!({"id": job}))).await;
    }
    let mut detached = Ok(());
    for node in &nodes {
        if let Err(e) = qmp_exec(
            sock,
            "blockdev-del",
            Some(serde_json::json!({"node-name": node})),
        )
        .await
        {
            detached = Err(e);
        }
    }
    run?;
    detached
}
