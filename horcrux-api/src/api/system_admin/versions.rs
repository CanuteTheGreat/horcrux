//! Versions of the platform pieces on this node (System > About):
//! kernel, QEMU, libvirt, Incus, LXC, LXD, Podman, Docker, OpenZFS, Ceph.
//!
//! * `GET /api/system/versions`
//!
//! Package versions come from the installed-package database (Portage's
//! `/var/db/pkg`, or `dpkg-query` on Debian-family hosts); what is actually
//! loaded or running is reported next to it where it can differ (the
//! running kernel, the loaded ZFS module), because an update installs the
//! new version while the old one keeps running until a restart.
use super::portage_tree as pt;
use super::{rooted, sysroot, which};
use crate::AppState;
use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/system/versions", get(versions))
}

/// (id, display name, Portage atom, Debian package)
pub const COMPONENTS: &[(&str, &str, &str, &str)] = &[
    ("kernel", "Linux kernel", "virtual/dist-kernel", "linux-image-amd64"),
    ("qemu", "QEMU", "app-emulation/qemu", "qemu-system-x86"),
    ("libvirt", "libvirt", "app-emulation/libvirt", "libvirt-daemon"),
    ("incus", "Incus", "app-containers/incus", "incus"),
    ("lxc", "LXC", "app-containers/lxc", "lxc"),
    ("lxd", "LXD", "app-containers/lxd", "lxd"),
    ("podman", "Podman", "app-containers/podman", "podman"),
    ("docker", "Docker", "app-containers/docker", "docker.io"),
    ("zfs", "OpenZFS", "sys-fs/zfs", "zfsutils-linux"),
    ("ceph", "Ceph", "sys-cluster/ceph", "ceph-common"),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub package: String,
    /// installed package version (`None`: not installed)
    pub version: Option<String>,
    /// what runs now when it differs from the package (running kernel,
    /// loaded ZFS module)
    pub running: Option<String>,
    /// extra facts (Ceph release name, the kernel line)
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Versions {
    pub horcrux: String,
    /// `portage` | `apt` | `unknown`
    pub package_db: String,
    pub components: Vec<Component>,
}

fn dpkg_version(pkg: &str) -> Option<String> {
    let out = std::process::Command::new("dpkg-query")
        .args(["-W", "-f=${Status}|${Version}", pkg])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let (status, ver) = s.split_once('|')?;
    (status.ends_with("installed") && !ver.is_empty()).then(|| ver.to_string())
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn gather(root: &Path, portage: bool, apt: bool) -> Versions {
    let line = super::kernel_line::load().line;
    let components = COMPONENTS
        .iter()
        .map(|(id, name, atom, deb)| {
            let (package, mut version) = if portage {
                (atom.to_string(), pt::installed(root, atom).into_iter().next())
            } else if apt {
                (deb.to_string(), dpkg_version(deb))
            } else {
                (atom.to_string(), None)
            };
            let mut running = None;
            let mut note = None;
            match *id {
                "kernel" => {
                    if portage && version.is_none() {
                        version = super::kernel_line::KERNEL_PACKAGES[..2]
                            .iter()
                            .find_map(|p| pt::installed(root, p).into_iter().next());
                    }
                    running = read_trim(&root.join("proc/sys/kernel/osrelease"))
                        .or_else(|| read_trim(Path::new("/proc/sys/kernel/osrelease")));
                    note = Some(format!("kernel line: {}", line.as_str()));
                }
                "zfs" => {
                    running = read_trim(&root.join("sys/module/zfs/version"))
                        .map(|v| format!("module {v}"));
                }
                "ceph" => {
                    note = version
                        .as_deref()
                        .map(|v| crate::api::ceph_install::release_label(pt::major_minor(v).0));
                }
                _ => {}
            }
            Component {
                id: id.to_string(),
                name: name.to_string(),
                package,
                version,
                running,
                note,
            }
        })
        .collect();
    Versions {
        horcrux: env!("CARGO_PKG_VERSION").into(),
        package_db: if portage {
            "portage"
        } else if apt {
            "apt"
        } else {
            "unknown"
        }
        .into(),
        components,
    }
}

async fn versions() -> Json<Versions> {
    let portage = rooted("var/db/pkg").is_dir();
    let apt = !portage && which("dpkg-query").is_some();
    Json(gather(&sysroot(), portage, apt))
}

#[cfg(test)]
mod tests {
    use super::super::portage_tree::tests::vdb;
    use super::*;

    #[test]
    fn versions_come_from_the_vdb_with_running_facts() {
        let t = tempfile::tempdir().unwrap();
        vdb(t.path(), "app-emulation/qemu", "11.1.3", "");
        vdb(t.path(), "app-containers/incus", "7.6", "");
        vdb(t.path(), "app-containers/lxc", "7.0.0-r1", "");
        vdb(t.path(), "sys-fs/zfs", "2.4.4", "");
        vdb(t.path(), "sys-cluster/ceph", "21.2.0", "");
        vdb(t.path(), "sys-kernel/gentoo-kernel-bin", "6.18.64_p1", "");
        std::fs::create_dir_all(t.path().join("sys/module/zfs")).unwrap();
        std::fs::write(t.path().join("sys/module/zfs/version"), "2.4.4-1\n").unwrap();
        std::fs::create_dir_all(t.path().join("proc/sys/kernel")).unwrap();
        std::fs::write(t.path().join("proc/sys/kernel/osrelease"), "6.18.64-p1-gentoo-dist\n").unwrap();
        let v = gather(t.path(), true, false);
        let get = |id: &str| v.components.iter().find(|c| c.id == id).unwrap().clone();
        assert_eq!(v.package_db, "portage");
        assert_eq!(get("qemu").version.as_deref(), Some("11.1.3"));
        assert_eq!(get("incus").version.as_deref(), Some("7.6"));
        assert_eq!(get("lxc").version.as_deref(), Some("7.0.0-r1"));
        assert_eq!(get("lxd").version, None);
        assert_eq!(get("zfs").running.as_deref(), Some("module 2.4.4-1"));
        assert_eq!(get("ceph").note.as_deref(), Some("Umbrella (21)"));
        let k = get("kernel");
        assert_eq!(k.version.as_deref(), Some("6.18.64_p1"));
        assert_eq!(k.running.as_deref(), Some("6.18.64-p1-gentoo-dist"));
    }
}
