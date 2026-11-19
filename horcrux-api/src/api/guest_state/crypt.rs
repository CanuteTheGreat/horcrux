//! Saved guest memory contains secrets (disk-encryption keys of an unlocked
//! guest, TLS keys, passwords). It may therefore only be written to storage
//! that is encrypted at rest: a ZFS dataset with native encryption (key
//! loaded) or a LUKS/dm-crypt volume. This module finds out what a target
//! directory lives on and decides whether a save may go there.
//!
//! Parsing and the decision are pure (unit-tested); only [`probe`] runs the
//! host tools (`findmnt`, `zfs`, `lsblk`).
use super::policy::SaveEncryption;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// What a directory is stored on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Probe {
    /// zfs-native | luks | none | unknown
    pub kind: String,
    /// true only when the data is encrypted at rest *and* persistent.
    pub encrypted: bool,
    /// Human-readable detail ("ZFS dataset rpool/x, aes-256-gcm", ...).
    pub detail: String,
}

impl Probe {
    fn yes(kind: &str, detail: String) -> Self {
        Probe {
            kind: kind.into(),
            encrypted: true,
            detail,
        }
    }
    fn no(detail: String) -> Self {
        Probe {
            kind: "none".into(),
            encrypted: false,
            detail,
        }
    }
    fn unknown(detail: String) -> Self {
        Probe {
            kind: "unknown".into(),
            encrypted: false,
            detail,
        }
    }
}

/// `findmnt -n -o SOURCE,FSTYPE -T dir` -> (source, fstype).
pub fn parse_findmnt(out: &str) -> Option<(String, String)> {
    let line = out.lines().find(|l| !l.trim().is_empty())?;
    let mut it = line.split_whitespace();
    let src = it.next()?.to_string();
    let fs = it.next()?.to_string();
    Some((src, fs))
}

/// `zfs get -H -o value encryption,keystatus DATASET` -> (encryption, keystatus).
pub fn parse_zfs_props(out: &str) -> Option<(String, String)> {
    let mut v = out.lines().map(|l| l.trim().to_string());
    let enc = v.next()?;
    let key = v.next()?;
    Some((enc, key))
}

/// `lsblk -s -n -o TYPE DEV` (the device and everything below it) mentions
/// `crypt` when a dm-crypt/LUKS layer is in the stack.
pub fn lsblk_has_crypt(out: &str) -> bool {
    out.lines().any(|l| l.trim() == "crypt")
}

/// A mount source with a btrfs subvolume suffix (`/dev/mapper/x[/@]`).
pub fn strip_subvol(src: &str) -> &str {
    src.split('[').next().unwrap_or(src)
}

/// Filesystems that vanish at reboot or live somewhere else entirely: a
/// save there cannot be resumed, or is not under our control.
pub fn volatile_or_remote(fstype: &str) -> Option<&'static str> {
    match fstype {
        "tmpfs" | "ramfs" | "devtmpfs" => Some("memory-backed (lost at reboot)"),
        "overlay" | "aufs" => Some("a container overlay (not the host's own storage)"),
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "virtiofs" | "fuse.sshfs" => {
            Some("network storage (encryption at rest is not under the host's control)")
        }
        _ => None,
    }
}

/// The nearest existing ancestor (or the path itself).
fn existing(dir: &Path) -> &Path {
    let mut p = dir;
    while !p.exists() {
        match p.parent() {
            Some(q) => p = q,
            None => break,
        }
    }
    p
}

async fn run(bin: &str, args: &[&str]) -> Option<String> {
    let fut = tokio::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output();
    let o = tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .ok()?
        .ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// What `dir` is stored on (the directory need not exist yet).
pub async fn probe(dir: &Path) -> Probe {
    let at = existing(dir).to_string_lossy().into_owned();
    let Some(out) = run("findmnt", &["-n", "-o", "SOURCE,FSTYPE", "-T", &at]).await else {
        return Probe::unknown(format!(
            "cannot tell what {at} is stored on (findmnt failed)"
        ));
    };
    let Some((src, fs)) = parse_findmnt(&out) else {
        return Probe::unknown(format!("cannot tell what {at} is stored on"));
    };
    if let Some(why) = volatile_or_remote(&fs) {
        return Probe::no(format!("{at} is on {fs}: {why}"));
    }
    if fs == "zfs" {
        let ds = src.as_str();
        return match run(
            "zfs",
            &["get", "-H", "-o", "value", "encryption,keystatus", ds],
        )
        .await
        .and_then(|o| parse_zfs_props(&o))
        {
            Some((enc, key)) if enc != "off" && enc != "-" && key == "available" => {
                Probe::yes("zfs-native", format!("ZFS dataset {ds}, {enc}"))
            }
            Some((enc, key)) if enc != "off" && enc != "-" => Probe::no(format!(
                "ZFS dataset {ds} is encrypted ({enc}) but its key is {key}"
            )),
            Some(_) => Probe::no(format!("ZFS dataset {ds} is not encrypted")),
            None => Probe::unknown(format!("cannot read the encryption of ZFS dataset {ds}")),
        };
    }
    let dev = strip_subvol(&src);
    if dev.starts_with("/dev/") {
        return match run("lsblk", &["-s", "-n", "-o", "TYPE", dev]).await {
            Some(o) if lsblk_has_crypt(&o) => {
                Probe::yes("luks", format!("{dev} ({fs}) on a LUKS/dm-crypt volume"))
            }
            Some(_) => Probe::no(format!("{at} is on {dev} ({fs}), which is not encrypted")),
            None => Probe::unknown(format!("cannot inspect the block device {dev}")),
        };
    }
    Probe::no(format!("{at} is on {src} ({fs}), which is not encrypted"))
}

/// May a state be written to this target under `policy`? `Err` carries the
/// reason shown to the administrator and logged.
pub fn verdict(p: &Probe, policy: SaveEncryption) -> Result<(), String> {
    if p.encrypted || policy == SaveEncryption::AllowUnencrypted {
        return Ok(());
    }
    Err(format!(
        "saved memory can contain secrets and must only be written to encrypted storage, but {}. \
         Point 'Save states to' at a natively encrypted ZFS dataset or a LUKS volume \
         (or, knowing the risk, set save_encryption to allow_unencrypted)",
        p.detail
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findmnt_output_is_parsed() {
        assert_eq!(
            parse_findmnt("rpool/ROOT/gentoo zfs\n"),
            Some(("rpool/ROOT/gentoo".into(), "zfs".into()))
        );
        assert_eq!(
            parse_findmnt("/dev/mapper/cryptroot[/@] btrfs\n"),
            Some(("/dev/mapper/cryptroot[/@]".into(), "btrfs".into()))
        );
        assert_eq!(parse_findmnt(""), None);
    }

    #[test]
    fn zfs_props_are_parsed() {
        assert_eq!(
            parse_zfs_props("aes-256-gcm\navailable\n"),
            Some(("aes-256-gcm".into(), "available".into()))
        );
        assert_eq!(parse_zfs_props("off\n-\n").unwrap().0, "off");
        assert_eq!(parse_zfs_props("off"), None);
    }

    #[test]
    fn crypt_layers_are_found() {
        assert!(lsblk_has_crypt("crypt\npart\ndisk\n"));
        assert!(!lsblk_has_crypt("part\ndisk\n"));
        assert!(!lsblk_has_crypt(""));
        assert_eq!(strip_subvol("/dev/mapper/x[/@home]"), "/dev/mapper/x");
        assert_eq!(strip_subvol("/dev/sda1"), "/dev/sda1");
    }

    #[test]
    fn volatile_and_remote_filesystems_are_refused() {
        assert!(volatile_or_remote("tmpfs").is_some());
        assert!(volatile_or_remote("overlay").is_some());
        assert!(volatile_or_remote("nfs4").is_some());
        assert!(volatile_or_remote("ext4").is_none());
        assert!(volatile_or_remote("zfs").is_none());
    }

    #[test]
    fn unencrypted_targets_are_refused_unless_opted_in() {
        let enc = Probe::yes("luks", "x".into());
        let plain = Probe::no("/var is on /dev/sda1 (ext4), which is not encrypted".into());
        let unk = Probe::unknown("cannot tell".into());
        assert!(verdict(&enc, SaveEncryption::Require).is_ok());
        let e = verdict(&plain, SaveEncryption::Require).unwrap_err();
        assert!(e.contains("encrypted storage") && e.contains("not encrypted"));
        assert!(verdict(&unk, SaveEncryption::Require).is_err());
        assert!(verdict(&plain, SaveEncryption::AllowUnencrypted).is_ok());
        assert!(verdict(&unk, SaveEncryption::AllowUnencrypted).is_ok());
    }
}
