//! Snapshots as a volume chain on thick LVM (Proxmox VE 9 "snapshot as
//! volume chain"): each volume is a qcow2 image written straight onto its
//! LV. A snapshot freezes the current LV under a snapshot name and puts a
//! new qcow2 overlay LV (backing file = the frozen LV) in its place, so a
//! shared (lvmlockd) VG on a SAN LUN gets snapshots without LVM CoW
//! snapshots, which shared VGs do not support across nodes.
//!
//! Layout for volume `vm-100-disk-0` with snapshots `s1` then `s2`:
//!
//! ```text
//! snap_vm-100-disk-0_s1   qcow2, no backing          (oldest)
//! snap_vm-100-disk-0_s2   qcow2, backing = ..._s1
//! vm-100-disk-0           qcow2, backing = ..._s2    (current, the VM writes here)
//! ```
//!
//! * snapshot `s3`: `lvrename vm-100-disk-0 snap_vm-100-disk-0_s3`, a new
//!   `vm-100-disk-0` LV, `qemu-img create -b ..._s3`.
//! * rollback: only to the newest snapshot (as Proxmox): the current LV is
//!   removed (wipe/discard per pool options) and a fresh overlay on that
//!   snapshot takes its place. Older snapshots: delete the newer ones first.
//! * delete snapshot `sK`: its child (the next snapshot, or the current LV)
//!   is rebased onto sK's parent (`qemu-img rebase`, which copies sK's data
//!   into the child), then sK's LV is removed.
//!
//! Each LV is sized from `qemu-img measure` (fully allocated qcow2), so an
//! overlay can always hold every cluster of the disk. These operations work
//! on volumes no running VM has open (the caller checks); the planners are
//! pure and unit-tested, the executor runs them in order and stops at the
//! first failure.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Prefix of snapshot LVs.
pub const SNAP_PREFIX: &str = "snap_";
/// Prefix of an overlay LV while a snapshot or rollback builds it.
pub const TMP_PREFIX: &str = "hxnew_";

/// `snap_<volume>_<snapshot>`.
pub fn snap_lv(volume: &str, snap: &str) -> String {
    format!("{SNAP_PREFIX}{volume}_{snap}")
}

/// The snapshot name of `lv` if it is a snapshot LV of `volume`.
pub fn snap_of<'a>(volume: &str, lv: &'a str) -> Option<&'a str> {
    lv.strip_prefix(SNAP_PREFIX)?
        .strip_prefix(volume)?
        .strip_prefix('_')
        .filter(|s| !s.is_empty())
}

/// Is `lv` a snapshot or in-progress overlay LV (hidden from the volume
/// list, part of its volume)?
pub fn is_snap_lv(lv: &str) -> bool {
    lv.starts_with(SNAP_PREFIX) || lv.starts_with(TMP_PREFIX)
}

pub fn dev(vg: &str, lv: &str) -> String {
    format!("/dev/{vg}/{lv}")
}

/// One command: program + arguments.
pub type Cmd = Vec<String>;

fn c(args: &[&str]) -> Cmd {
    args.iter().map(|s| s.to_string()).collect()
}

/// LV size in bytes for a qcow2 image of `virtual_bytes` whose fully
/// allocated size `qemu-img measure` reported as `measured`, rounded up to
/// whole MiB (LVM rounds further to its extent size).
pub fn lv_bytes(measured: u64) -> u64 {
    const MIB: u64 = 1 << 20;
    measured.div_ceil(MIB) * MIB
}

/// Parse `qemu-img measure --output=json` (`fully-allocated`).
pub fn parse_measure(json: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("fully-allocated")?
        .as_u64()
}

/// New volume: LV + qcow2 on it.
pub fn plan_create(vg: &str, volume: &str, virtual_bytes: u64, lv_size: u64) -> Vec<Cmd> {
    vec![
        c(&[
            "lvcreate",
            "-y",
            "-W",
            "y",
            "-L",
            &format!("{lv_size}b"),
            "-n",
            volume,
            vg,
        ]),
        c(&[
            "qemu-img",
            "create",
            "-q",
            "-f",
            "qcow2",
            &dev(vg, volume),
            &virtual_bytes.to_string(),
        ]),
    ]
}

/// Name of the overlay LV while it is being built (renamed to the volume's
/// own name as the last step, so a failure never leaves the volume gone).
pub fn tmp_lv(volume: &str) -> String {
    format!("{TMP_PREFIX}{volume}")
}

fn lvcreate(vg: &str, lv: &str, lv_size: u64) -> Cmd {
    c(&[
        "lvcreate",
        "-y",
        "-W",
        "y",
        "-L",
        &format!("{lv_size}b"),
        "-n",
        lv,
        vg,
    ])
}

fn overlay(vg: &str, lv: &str, backing_lv: &str, virtual_bytes: u64) -> Cmd {
    c(&[
        "qemu-img",
        "create",
        "-q",
        "-f",
        "qcow2",
        "-b",
        &dev(vg, backing_lv),
        "-F",
        "qcow2",
        &dev(vg, lv),
        &virtual_bytes.to_string(),
    ])
}

/// The steps of a snapshot, in order, each with what undoes everything
/// done before it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub cmd: Cmd,
    /// Commands that put the chain back when THIS step fails.
    pub undo: Vec<Cmd>,
}

/// Snapshot `snap` of `volume`: the overlay LV is created first under a
/// temporary name (no space: nothing changed), then the current LV is
/// frozen under the snapshot name, the overlay is written on the temporary
/// LV, and it takes the volume's name.
pub fn plan_snapshot(
    vg: &str,
    volume: &str,
    snap: &str,
    virtual_bytes: u64,
    lv_size: u64,
) -> Vec<Step> {
    let s = snap_lv(volume, snap);
    let t = tmp_lv(volume);
    let rm_tmp = c(&["lvremove", "-y", &format!("{vg}/{t}")]);
    let unfreeze = c(&["lvrename", vg, &s, volume]);
    vec![
        Step {
            cmd: lvcreate(vg, &t, lv_size),
            undo: vec![],
        },
        Step {
            cmd: c(&["lvrename", vg, volume, &s]),
            undo: vec![rm_tmp.clone()],
        },
        Step {
            cmd: overlay(vg, &t, &s, virtual_bytes),
            undo: vec![rm_tmp.clone(), unfreeze.clone()],
        },
        Step {
            cmd: c(&["lvrename", vg, &t, volume]),
            undo: vec![rm_tmp, unfreeze],
        },
    ]
}

/// Rollback to the newest snapshot: a fresh overlay on `snap` is built
/// under the temporary name first; the caller then removes the current LV
/// (with its scrub options) and runs the returned final rename.
pub fn plan_rollback_overlay(
    vg: &str,
    volume: &str,
    snap: &str,
    virtual_bytes: u64,
    lv_size: u64,
) -> (Vec<Cmd>, Cmd) {
    let s = snap_lv(volume, snap);
    let t = tmp_lv(volume);
    (
        vec![
            lvcreate(vg, &t, lv_size),
            overlay(vg, &t, &s, virtual_bytes),
        ],
        c(&["lvrename", vg, &t, volume]),
    )
}

/// `vgs -o vg_free --units b --nosuffix` output -> bytes.
pub fn parse_vg_free(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.trim().parse::<u64>().ok())
}

/// Delete snapshot `chain[idx]` (chain oldest first, without the current
/// LV): rebase its child onto its parent. Returns the rebase command; the
/// caller removes the snapshot LV after it succeeded.
pub fn plan_delete_rebase(vg: &str, volume: &str, chain: &[String], idx: usize) -> Option<Cmd> {
    chain.get(idx)?;
    let child = match chain.get(idx + 1) {
        Some(n) => snap_lv(volume, n),
        None => volume.to_string(),
    };
    let parent = if idx == 0 {
        String::new()
    } else {
        dev(vg, &snap_lv(volume, &chain[idx - 1]))
    };
    let mut v = c(&["qemu-img", "rebase", "-f", "qcow2", "-b", &parent]);
    if !parent.is_empty() {
        v.extend(c(&["-F", "qcow2"]));
    }
    v.push(dev(vg, &child));
    Some(v)
}

/// The snapshot chain of `volume`, oldest first, from
/// `qemu-img info --backing-chain --output=json <current LV>` (a JSON
/// array, current image first). Images that are not this volume's
/// snapshot LVs end the walk.
pub fn parse_chain(volume: &str, json: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("qemu-img info: {e}"))?;
    let arr = v
        .as_array()
        .ok_or("qemu-img info --backing-chain: not a list")?;
    let mut out = Vec::new();
    for img in arr.iter().skip(1) {
        let f = img.get("filename").and_then(|f| f.as_str()).unwrap_or("");
        let lv = f.rsplit('/').next().unwrap_or("");
        match snap_of(volume, lv) {
            Some(s) => out.push(s.to_string()),
            None => break,
        }
    }
    out.reverse();
    Ok(out)
}

/// The virtual size from `qemu-img info --output=json`.
pub fn parse_virtual_size(json: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let v = if let Some(a) = v.as_array() {
        a.first()?.clone()
    } else {
        v
    };
    v.get("virtual-size")?.as_u64()
}

/// A volume's snapshots as the API shows them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainView {
    pub volume: String,
    /// Oldest first; the last one is what a rollback returns to.
    pub snapshots: Vec<String>,
    pub virtual_size_bytes: Option<u64>,
}

// ---------------------------------------------------------------- executor

async fn run(cmd: &Cmd, t: Duration) -> Result<String, String> {
    let r = crate::san::iscsi::run(
        &cmd[0],
        &cmd[1..].iter().map(String::as_str).collect::<Vec<_>>(),
        t,
    )
    .await?;
    if r.ok {
        Ok(r.stdout)
    } else {
        Err(format!("{}: {}", cmd.join(" "), r.stderr.trim()))
    }
}

pub async fn run_all(cmds: &[Cmd], t: Duration) -> Result<(), String> {
    for x in cmds {
        run(x, t).await?;
    }
    Ok(())
}

/// Fully allocated qcow2 size for `virtual_bytes` (`qemu-img measure`).
pub async fn measure(virtual_bytes: u64, t: Duration) -> Result<u64, String> {
    let out = run(
        &c(&[
            "qemu-img",
            "measure",
            "--output=json",
            "-O",
            "qcow2",
            "--size",
            &virtual_bytes.to_string(),
        ]),
        t,
    )
    .await?;
    parse_measure(&out)
        .map(lv_bytes)
        .ok_or_else(|| format!("qemu-img measure: no fully-allocated size in {out:?}"))
}

/// Make every LV of the chain usable on this node (exclusive activation on
/// a shared VG, plain activation otherwise).
pub async fn activate(vg: &str, lvs: &[String], shared: bool, t: Duration) -> Result<(), String> {
    for lv in lvs {
        if shared {
            crate::san::lvm_shared::activate_exclusive(vg, lv, t).await?;
        } else {
            run(&c(&["lvchange", "-ay", &format!("{vg}/{lv}")]), t).await?;
        }
    }
    Ok(())
}

pub async fn info_chain(vg: &str, volume: &str, t: Duration) -> Result<ChainView, String> {
    let out = run(
        &c(&[
            "qemu-img",
            "info",
            "-U",
            "--backing-chain",
            "--output=json",
            &dev(vg, volume),
        ]),
        t,
    )
    .await?;
    Ok(ChainView {
        volume: volume.to_string(),
        snapshots: parse_chain(volume, &out)?,
        virtual_size_bytes: parse_virtual_size(&out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(snap_lv("vm-100-disk-0", "s1"), "snap_vm-100-disk-0_s1");
        assert_eq!(
            snap_of("vm-100-disk-0", "snap_vm-100-disk-0_s1"),
            Some("s1")
        );
        assert_eq!(snap_of("vm-100-disk-0", "snap_vm-100-disk-0_"), None);
        assert_eq!(snap_of("vm-100-disk-0", "snap_vm-100-disk-01_s1"), None);
        assert_eq!(snap_of("vm-100-disk-0", "vm-100-disk-0"), None);
        assert!(is_snap_lv("snap_x_y"));
        assert!(!is_snap_lv("vm-1"));
    }

    #[test]
    fn measure_and_sizes() {
        let j = r#"{"bitmaps": 0, "required": 393216, "fully-allocated": 1074135040}"#;
        assert_eq!(parse_measure(j), Some(1_074_135_040));
        assert_eq!(lv_bytes(1_074_135_040), 1025 << 20);
        assert_eq!(lv_bytes(1 << 20), 1 << 20);
        assert_eq!(parse_measure("{}"), None);
    }

    #[test]
    fn create_and_snapshot_plans() {
        let p = plan_create("vg0", "vm-1-disk-0", 1 << 30, 1025 << 20);
        assert_eq!(p[0][0], "lvcreate");
        assert!(p[0].contains(&format!("{}b", 1025u64 << 20)));
        assert_eq!(
            p[1],
            c(&[
                "qemu-img",
                "create",
                "-q",
                "-f",
                "qcow2",
                "/dev/vg0/vm-1-disk-0",
                "1073741824"
            ])
        );
        let s = plan_snapshot("vg0", "vm-1-disk-0", "s1", 1 << 30, 1025 << 20);
        assert_eq!(s.len(), 4);
        // The overlay LV exists before anything is renamed.
        assert_eq!(s[0].cmd[0], "lvcreate");
        assert!(s[0].cmd.contains(&"hxnew_vm-1-disk-0".to_string()));
        assert!(s[0].undo.is_empty());
        assert_eq!(
            s[1].cmd,
            c(&["lvrename", "vg0", "vm-1-disk-0", "snap_vm-1-disk-0_s1"])
        );
        assert_eq!(
            s[1].undo,
            vec![c(&["lvremove", "-y", "vg0/hxnew_vm-1-disk-0"])]
        );
        assert!(s[2]
            .cmd
            .windows(2)
            .any(|w| w[0] == "-b" && w[1] == "/dev/vg0/snap_vm-1-disk-0_s1"));
        assert!(s[2].cmd.contains(&"/dev/vg0/hxnew_vm-1-disk-0".to_string()));
        assert_eq!(s[2].cmd.last().unwrap(), "1073741824");
        // A failed overlay puts the frozen LV back under the volume's name.
        assert!(s[2].undo.contains(&c(&[
            "lvrename",
            "vg0",
            "snap_vm-1-disk-0_s1",
            "vm-1-disk-0"
        ])));
        assert_eq!(
            s[3].cmd,
            c(&["lvrename", "vg0", "hxnew_vm-1-disk-0", "vm-1-disk-0"])
        );
        let (build, fin) = plan_rollback_overlay("vg0", "v", "s2", 10, 1 << 20);
        assert!(build[1].iter().any(|a| a == "/dev/vg0/snap_v_s2"));
        assert!(build[1].iter().any(|a| a == "/dev/vg0/hxnew_v"));
        assert_eq!(fin, c(&["lvrename", "vg0", "hxnew_v", "v"]));
        assert_eq!(parse_vg_free("  1073741824\n"), Some(1 << 30));
        assert_eq!(parse_vg_free(""), None);
        assert!(is_snap_lv("hxnew_v"));
    }

    #[test]
    fn delete_rebases_the_child_onto_the_parent() {
        let chain = vec!["s1".to_string(), "s2".to_string(), "s3".to_string()];
        // Middle: s3 rebased onto s1.
        assert_eq!(
            plan_delete_rebase("vg", "v", &chain, 1).unwrap(),
            c(&[
                "qemu-img",
                "rebase",
                "-f",
                "qcow2",
                "-b",
                "/dev/vg/snap_v_s1",
                "-F",
                "qcow2",
                "/dev/vg/snap_v_s3"
            ])
        );
        // Newest: the current LV rebased onto s2.
        assert_eq!(
            plan_delete_rebase("vg", "v", &chain, 2).unwrap(),
            c(&[
                "qemu-img",
                "rebase",
                "-f",
                "qcow2",
                "-b",
                "/dev/vg/snap_v_s2",
                "-F",
                "qcow2",
                "/dev/vg/v"
            ])
        );
        // Oldest: s2 gets all of s1's data and no backing file.
        assert_eq!(
            plan_delete_rebase("vg", "v", &chain, 0).unwrap(),
            c(&[
                "qemu-img",
                "rebase",
                "-f",
                "qcow2",
                "-b",
                "",
                "/dev/vg/snap_v_s2"
            ])
        );
        assert!(plan_delete_rebase("vg", "v", &chain, 3).is_none());
    }

    #[test]
    fn chain_from_qemu_img_info() {
        let j = r#"[
          {"filename": "/dev/vg0/v", "format": "qcow2", "virtual-size": 1073741824,
           "backing-filename": "/dev/vg0/snap_v_s2"},
          {"filename": "/dev/vg0/snap_v_s2", "format": "qcow2", "virtual-size": 1073741824},
          {"filename": "/dev/vg0/snap_v_s1", "format": "qcow2", "virtual-size": 1073741824}
        ]"#;
        assert_eq!(parse_chain("v", j).unwrap(), vec!["s1", "s2"]);
        assert_eq!(parse_virtual_size(j), Some(1 << 30));
        let lone = r#"[{"filename": "/dev/vg0/v", "virtual-size": 5}]"#;
        assert!(parse_chain("v", lone).unwrap().is_empty());
        let foreign = r#"[{"filename": "/dev/vg0/v"}, {"filename": "/var/lib/x.qcow2"}]"#;
        assert!(parse_chain("v", foreign).unwrap().is_empty());
        assert!(parse_chain("v", "{}").is_err());
    }
}
