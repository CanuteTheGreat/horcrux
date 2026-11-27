//! Shared LVM on a SAN LUN: lvmlockd with sanlock leases on the LUN itself.
//!
//! Every node runs `lvmlockd` + `sanlock` with a cluster-unique sanlock
//! `host_id` (1..2000, see [`super::pick_host_id`]). Horcrux owns
//! `/etc/lvm/lvmlocal.conf` on such nodes (`use_lvmlockd = 1` and the
//! host id) and, only on test beds without a watchdog device,
//! `/etc/sanlock/sanlock.conf`. The VG is created once with
//! `vgcreate --shared --locktype sanlock`, every node runs
//! `vgchange --lockstart`, and LVs are activated exclusively (`-aey`) on
//! the node that uses them, so two nodes can never write one LV at once.
//!
//! Removing a volume can first zero it (`wipe_removed`) and/or discard its
//! extents (`discard_removed`), per storage.

use std::path::Path;
use std::time::Duration;

pub const HEADER: &str =
    "# Managed by Horcrux (shared LVM on SAN storage). Do not edit: changes are overwritten.";
pub const LVMLOCAL_PATH: &str = "/etc/lvm/lvmlocal.conf";
pub const SANLOCK_CONF_PATH: &str = "/etc/sanlock/sanlock.conf";

/// `lvmlocal.conf` for a node with this sanlock host id.
pub fn render_lvmlocal(host_id: u16) -> String {
    format!("{HEADER}\nglobal {{\n\tuse_lvmlockd = 1\n}}\nlocal {{\n\thost_id = {host_id}\n}}\n")
}

/// `sanlock.conf` (only written when the watchdog is turned off).
pub fn render_sanlock_conf(watchdog: bool) -> Option<String> {
    if watchdog {
        None
    } else {
        Some(format!("{HEADER}\nuse_watchdog = 0\n"))
    }
}

/// A file with no settings: comments, blank lines and empty sections only
/// (the packaged `lvmlocal.conf` is a commented `local { }`).
pub fn only_comments(text: &str) -> bool {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|t| !t.is_empty())
        .flat_map(|t| t.split_whitespace())
        .all(|w| matches!(w, "{" | "}" | "local" | "local{" | "global" | "global{"))
}

/// Write a Horcrux-owned config file. A packaged comments-only default is
/// kept as `<file>.horcrux-orig`; a file with real admin settings is never
/// replaced (error: the admin must merge it by hand once).
pub fn write_owned(path: &Path, content: &str) -> Result<bool, String> {
    match std::fs::read_to_string(path) {
        Ok(old) if old == content => return Ok(false),
        Ok(old) if old.starts_with(HEADER) => {}
        Ok(old) if only_comments(&old) => {
            let keep = path.with_extension("conf.horcrux-orig");
            if !keep.exists() {
                std::fs::write(&keep, old).map_err(|e| format!("{}: {e}", keep.display()))?;
            }
        }
        Ok(_) => {
            return Err(format!(
                "{} has settings that are not Horcrux's; shared LVM needs Horcrux to own it (move the settings to lvm.conf)",
                path.display()
            ))
        }
        Err(_) => {}
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("horcrux-tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// Remove a Horcrux-owned file, restoring the packaged original if kept.
pub fn remove_owned(path: &Path) -> Result<bool, String> {
    match std::fs::read_to_string(path) {
        Ok(old) if old.starts_with(HEADER) => {
            let keep = path.with_extension("conf.horcrux-orig");
            if keep.exists() {
                std::fs::rename(&keep, path).map_err(|e| format!("{}: {e}", path.display()))?;
            } else {
                std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `vgs --noheadings -o vg_name,vg_lock_type,vg_lock_args <vg>` -> lock type
/// (`sanlock`, `dlm`, `none`/empty).
pub fn parse_vg_lock_type(text: &str, vg: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some(vg)).then(|| it.next().unwrap_or("none").to_string())
    })
}

/// What to do to a volume before `lvremove`, per the storage's options.
/// Each entry is a full argv.
pub fn removal_steps(lv_path: &str, wipe: bool, discard: bool) -> Vec<Vec<String>> {
    let mut v = Vec::new();
    if wipe {
        // BLKZEROOUT: the kernel writes zeroes (or uses WRITE SAME/ZEROES).
        v.push(vec![
            "blkdiscard".into(),
            "--zeroout".into(),
            lv_path.into(),
        ]);
    }
    if discard {
        v.push(vec!["blkdiscard".into(), lv_path.into()]);
    }
    v
}

/// Is `blkdiscard`'s failure only "the device cannot discard"? Then the
/// removal goes on (discard is best effort; a wipe failure is not).
pub fn discard_unsupported(stderr: &str) -> bool {
    stderr.contains("Operation not supported") || stderr.contains("BLKDISCARD ioctl failed")
}

async fn run(prog: &str, args: &[&str], t: Duration) -> Result<super::iscsi::Run, String> {
    super::iscsi::run(prog, args, t).await
}

fn fail(what: &str, r: &super::iscsi::Run) -> String {
    format!("{what}: {}", r.stderr.trim())
}

/// The VG's lock type, `None` when the VG is not visible here.
pub async fn vg_lock_type(vg: &str, t: Duration) -> Result<Option<String>, String> {
    let r = run(
        "vgs",
        &["--noheadings", "-o", "vg_name,vg_lock_type", vg],
        t,
    )
    .await?;
    if !r.ok {
        return Ok(None);
    }
    Ok(parse_vg_lock_type(&r.stdout, vg))
}

/// Create the shared VG on `dev` (once, from one node).
pub async fn create_shared_vg(vg: &str, dev: &str, t: Duration) -> Result<(), String> {
    // No separate pvcreate: it needs lvmlockd's global lock, which only
    // exists once the first shared VG does (lvmlockd(8), "Initial set up");
    // vgcreate initializes the device itself.
    let r = run(
        "vgcreate",
        &["--shared", "--locktype", "sanlock", vg, dev],
        t,
    )
    .await?;
    if !r.ok {
        return Err(fail("vgcreate --shared", &r));
    }
    Ok(())
}

/// Join this node to the VG's lockspace (idempotent).
pub async fn lockstart(vg: &str, t: Duration) -> Result<(), String> {
    let _ = run("pvscan", &["--cache"], t).await;
    let r = run("vgchange", &["--lockstart", vg], t).await?;
    if r.ok || r.stderr.contains("already started") || r.stdout.contains("already started") {
        Ok(())
    } else {
        Err(fail("vgchange --lockstart", &r))
    }
}

/// Leave the VG's lockspace on this node (storage removed here).
pub async fn lockstop(vg: &str, t: Duration) -> Result<(), String> {
    let r = run("vgchange", &["--lockstop", vg], t).await?;
    if r.ok {
        Ok(())
    } else {
        Err(fail("vgchange --lockstop", &r))
    }
}

/// Activate an LV exclusively on this node (`-aey`); a shared VG refuses
/// while another node holds it.
pub async fn activate_exclusive(vg: &str, lv: &str, t: Duration) -> Result<(), String> {
    let name = format!("{vg}/{lv}");
    let r = run("lvchange", &["-aey", &name], t).await?;
    if r.ok {
        Ok(())
    } else {
        Err(fail(&format!("lvchange -aey {name}"), &r))
    }
}

pub async fn deactivate(vg: &str, lv: &str, t: Duration) -> Result<(), String> {
    let name = format!("{vg}/{lv}");
    let r = run("lvchange", &["-an", &name], t).await?;
    if r.ok {
        Ok(())
    } else {
        Err(fail(&format!("lvchange -an {name}"), &r))
    }
}

/// Run [`removal_steps`] on an active LV.
pub async fn scrub_before_remove(
    lv_path: &str,
    wipe: bool,
    discard: bool,
    t: Duration,
) -> Result<(), String> {
    for argv in removal_steps(lv_path, wipe, discard) {
        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        let r = run(&argv[0], &args, t).await?;
        if !r.ok {
            let is_discard = !args.contains(&"--zeroout");
            if is_discard && discard_unsupported(&r.stderr) {
                tracing::info!(
                    "{lv_path}: the device does not support discard; removing without it"
                );
                continue;
            }
            return Err(fail(&argv.join(" "), &r));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lvmlocal() {
        let c = render_lvmlocal(7);
        assert!(c.starts_with(HEADER));
        assert!(c.contains("global {\n\tuse_lvmlockd = 1\n}\n"));
        assert!(c.ends_with("local {\n\thost_id = 7\n}\n"));
        assert_eq!(render_sanlock_conf(true), None);
        assert!(render_sanlock_conf(false)
            .unwrap()
            .contains("use_watchdog = 0"));
    }

    #[test]
    fn owned_files_keep_the_packaged_default_and_never_take_admin_settings() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("lvmlocal.conf");
        std::fs::write(&p, "# packaged default\n\n#local {\n#}\n").unwrap();
        assert!(write_owned(&p, &render_lvmlocal(1)).unwrap());
        assert!(d.path().join("lvmlocal.conf.horcrux-orig").exists());
        assert!(!write_owned(&p, &render_lvmlocal(1)).unwrap());
        assert!(write_owned(&p, &render_lvmlocal(2)).unwrap());
        assert!(remove_owned(&p).unwrap());
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "# packaged default\n\n#local {\n#}\n"
        );
        // the packaged lvmlocal.conf: a commented, empty local section
        let q = d.path().join("pkg.conf");
        std::fs::write(&q, "# lvmlocal.conf\nlocal {\n\t# host_id = 0\n}\n").unwrap();
        assert!(write_owned(&q, &render_lvmlocal(3)).unwrap());
        std::fs::write(&p, "local {\n\thost_id = 9\n}\n").unwrap();
        assert!(write_owned(&p, &render_lvmlocal(1)).is_err());
        assert!(!remove_owned(&p).unwrap());
    }

    #[test]
    fn vg_lock_type_parse() {
        let t = "  vg_san   sanlock\n  vg_local\n";
        assert_eq!(parse_vg_lock_type(t, "vg_san").as_deref(), Some("sanlock"));
        assert_eq!(parse_vg_lock_type(t, "vg_local").as_deref(), Some("none"));
        assert_eq!(parse_vg_lock_type(t, "vg_x"), None);
    }

    #[test]
    fn removal_order() {
        assert!(removal_steps("/dev/vg/lv", false, false).is_empty());
        assert_eq!(
            removal_steps("/dev/vg/lv", true, true),
            vec![
                vec![
                    "blkdiscard".to_string(),
                    "--zeroout".into(),
                    "/dev/vg/lv".into()
                ],
                vec!["blkdiscard".to_string(), "/dev/vg/lv".into()],
            ]
        );
        assert!(discard_unsupported(
            "blkdiscard: /dev/vg/lv: BLKDISCARD ioctl failed: Operation not supported"
        ));
        assert!(!discard_unsupported("blkdiscard: cannot open /dev/vg/lv"));
    }
}
