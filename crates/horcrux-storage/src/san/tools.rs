//! Which SAN host tools this node has, and the 424 reasons when one is
//! missing (`horcrux_common::Error::NodeLacks`).

use serde::{Deserialize, Serialize};
use std::path::Path;

const BIN_DIRS: &[&str] = &["/usr/sbin", "/sbin", "/usr/bin", "/bin", "/usr/local/sbin"];

/// Is `prog` on this host (absolute path or in the usual sbin/bin dirs)?
pub fn have(prog: &str) -> bool {
    have_in(prog, BIN_DIRS.iter().map(Path::new))
}

pub fn have_in<'a>(prog: &str, dirs: impl IntoIterator<Item = &'a Path>) -> bool {
    if prog.starts_with('/') {
        return Path::new(prog).exists();
    }
    dirs.into_iter().any(|d| d.join(prog).is_file())
}

/// Host tool availability, as the API's status view shows it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolStatus {
    pub iscsiadm: bool,
    pub multipathd: bool,
    pub multipath: bool,
    pub lvmlockd: bool,
    pub sanlock: bool,
    pub blkdiscard: bool,
    pub nvme: bool,
    /// qemu-img, for snapshots as a volume chain.
    #[serde(default)]
    pub qemu_img: bool,
}

impl ToolStatus {
    pub fn probe() -> Self {
        Self {
            iscsiadm: have("iscsiadm"),
            multipathd: have("multipathd"),
            multipath: have("multipath"),
            lvmlockd: have("lvmlockd"),
            sanlock: have("sanlock"),
            blkdiscard: have("blkdiscard"),
            nvme: have("nvme"),
            qemu_img: have("qemu-img"),
        }
    }
}

/// The kernel lacks the fabrics transport module (`nvme_tcp`/`nvme_rdma`).
pub fn nvme_transport_missing(module: &str, detail: &str) -> horcrux_common::Error {
    horcrux_common::Error::NodeLacks {
        reason: "san-nvme-transport-missing",
        message: format!("This node's kernel has no NVMe over Fabrics transport {module}: {detail}."),
        hint: "Build the kernel with CONFIG_NVME_TCP (or CONFIG_NVME_RDMA for RDMA) and load the module.".into(),
    }
}

/// No Fibre Channel HBA on this node (nothing under `/sys/class/fc_host`).
pub fn fc_no_hba(detail: &str) -> horcrux_common::Error {
    horcrux_common::Error::NodeLacks {
        reason: "san-fc-no-hba",
        message: format!("This node has no Fibre Channel HBA: {detail}."),
        hint: "Install an FC HBA and load its driver (qla2xxx, lpfc, ...); its ports then show up here with their WWPNs.".into(),
    }
}

/// A missing SAN facility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Iscsi,
    Multipath,
    SharedLvm,
    Discard,
    /// nvme-cli, for NVMe over Fabrics.
    Nvme,
    /// qemu-img, for snapshots as a volume chain on LVM.
    VolumeChain,
}

impl Need {
    /// `Err(NodeLacks)` when this node lacks what `self` needs.
    pub fn check(self, t: &ToolStatus) -> horcrux_common::Result<()> {
        let (ok, reason, message, hint) = match self {
            Need::Iscsi => (
                t.iscsiadm,
                "san-iscsiadm-missing",
                "This node has no iSCSI initiator (iscsiadm).",
                "Install open-iscsi (emerge sys-block/open-iscsi) and start iscsid, or enable USE=san on horcrux.",
            ),
            Need::Multipath => (
                t.multipathd && t.multipath,
                "san-multipathd-missing",
                "This node has no dm-multipath daemon (multipathd).",
                "Install multipath-tools (emerge sys-fs/multipath-tools), or turn multipath off in the SAN settings.",
            ),
            Need::SharedLvm => (
                t.lvmlockd && t.sanlock,
                "san-lvmlockd-missing",
                "This node cannot lock shared LVM volume groups (lvmlockd with sanlock is missing).",
                "Install sys-fs/lvm2 with USE=sanlock and sys-cluster/sanlock (USE=san on horcrux pulls both).",
            ),
            Need::Discard => (
                t.blkdiscard,
                "san-blkdiscard-missing",
                "This node has no blkdiscard, needed to wipe or discard removed volumes.",
                "Install sys-apps/util-linux, or turn wipe_removed/discard_removed off for this storage.",
            ),
            Need::Nvme => (
                t.nvme,
                "san-nvme-cli-missing",
                "This node has no NVMe over Fabrics tool (nvme-cli).",
                "Install sys-apps/nvme-cli (USE=san on horcrux pulls it).",
            ),
            Need::VolumeChain => (
                t.qemu_img,
                "san-qemu-img-missing",
                "This node has no qemu-img, needed for snapshots as a volume chain.",
                "Install app-emulation/qemu (it provides qemu-img), or create the storage without snapshot_as_volume_chain.",
            ),
        };
        if ok {
            Ok(())
        } else {
            Err(horcrux_common::Error::NodeLacks {
                reason,
                message: message.into(),
                hint: hint.into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_in_dirs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("iscsiadm"), "").unwrap();
        assert!(have_in("iscsiadm", [d.path()]));
        assert!(!have_in("multipathd", [d.path()]));
    }

    #[test]
    fn missing_tools_are_424_reasons() {
        let none = ToolStatus::default();
        for (n, reason) in [
            (Need::Iscsi, "san-iscsiadm-missing"),
            (Need::Multipath, "san-multipathd-missing"),
            (Need::SharedLvm, "san-lvmlockd-missing"),
            (Need::Discard, "san-blkdiscard-missing"),
            (Need::Nvme, "san-nvme-cli-missing"),
            (Need::VolumeChain, "san-qemu-img-missing"),
        ] {
            match n.check(&none) {
                Err(horcrux_common::Error::NodeLacks { reason: r, .. }) => assert_eq!(r, reason),
                other => panic!("{n:?}: {other:?}"),
            }
        }
        let all = ToolStatus {
            iscsiadm: true,
            multipathd: true,
            multipath: true,
            lvmlockd: true,
            sanlock: true,
            blkdiscard: true,
            nvme: true,
            qemu_img: true,
        };
        assert!(Need::SharedLvm.check(&all).is_ok());
    }
}
