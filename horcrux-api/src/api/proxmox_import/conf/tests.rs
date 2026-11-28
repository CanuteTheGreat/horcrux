use super::*;
use std::path::PathBuf;

const VM_LEGACY: &str = include_str!("../../../../tests/fixtures/proxmox/qemu-server/201.conf");
const VM_DNS: &str = include_str!("../../../../tests/fixtures/proxmox/qemu-server/202.conf");
const VM_ROUTER: &str = include_str!("../../../../tests/fixtures/proxmox/qemu-server/203.conf");
const CT_MIRROR: &str = include_str!("../../../../tests/fixtures/proxmox/lxc/210.conf");
const CT_TUN: &str = include_str!("../../../../tests/fixtures/proxmox/lxc/211.conf");
const CT_RATE: &str = include_str!("../../../../tests/fixtures/proxmox/lxc/212.conf");
const CT_VOLUME: &str = include_str!("../../../../tests/fixtures/proxmox/lxc/213.conf");
const SHOWCMD: &str = include_str!("../../../../tests/fixtures/proxmox/guests-qm-showcmd.txt");

/// Every volume resolves: zvols to /dev/zvol/..., subvols to /pool/...
fn resolve_all(v: &PveVol) -> Result<Resolved, String> {
    if v.volume.starts_with("subvol-") {
        Ok(Resolved {
            path: format!("/rpool/data/{}", v.volume),
            kind: VolKind::Dir,
            format: "dir".into(),
        })
    } else if v.storage.is_some() {
        Ok(Resolved {
            path: format!("/dev/zvol/rpool/data/{}", v.volume),
            kind: VolKind::Block,
            format: "raw".into(),
        })
    } else {
        Err("host path".into())
    }
}

fn host() -> HostCtx {
    HostCtx {
        bridges: vec!["vmbr0".into(), "vmbr2".into()],
        pci: [("0000:03:00.0".to_string(), Some(33))]
            .into_iter()
            .collect(),
        ovmf_secboot: true,
        ..Default::default()
    }
}

fn row<'a>(p: &'a GuestPlan, key: &str) -> &'a Row {
    p.rows
        .iter()
        .find(|r| r.key == key)
        .unwrap_or_else(|| panic!("no row {key}: {:#?}", p.rows))
}

/// Nothing is dropped silently: every key of the current section is a row.
fn every_key_has_a_row(conf: &PveConf, p: &GuestPlan) {
    for (k, _) in &conf.keys {
        assert!(
            p.rows.iter().any(|r| &r.key == k),
            "{k} of guest {} has no row",
            p.vmid
        );
    }
    assert!(
        !p.rows.iter().any(|r| r.note == "unknown key"),
        "unknown keys: {:?}",
        p.rows
            .iter()
            .filter(|r| r.note == "unknown key")
            .collect::<Vec<_>>()
    );
}

#[test]
fn conf_takes_the_current_section_and_decodes_the_description() {
    let c = parse_conf(VM_ROUTER);
    assert_eq!(c.get("name"), Some("edge-router"));
    assert_eq!(c.snapshots, vec!["kernel-update".to_string()]);
    assert!(c.description.starts_with("Edge router: WAN"));
    assert_eq!(c.keys.iter().filter(|(k, _)| k == "name").count(), 1);
    assert_eq!(pct_decode("a%20b%3Ac%0A%zz"), "a b:c\n%zz");
}

#[test]
fn volumes_parse() {
    let v = PveVol::parse("local-zfs:vm-202-disk-0,discard=on,iothread=1,mbps_rd=100,size=32G");
    assert_eq!(v.storage.as_deref(), Some("local-zfs"));
    assert_eq!(v.volume, "vm-202-disk-0");
    assert_eq!(v.size_bytes(), Some(32 << 30));
    assert!(PveVol::parse("none,media=cdrom").is_empty_drive());
    let b = PveVol::parse("/tank/x,mp=/srv/x,backup=0");
    assert!(b.storage.is_none() && b.volume == "/tank/x");
    assert_eq!(parse_size("512M"), Some(512 << 20));
    assert_eq!(pci_addr("ad:00.0"), "0000:ad:00.0");
    assert_eq!(pci_addr("0000:03:00"), "0000:03:00.0");
}

#[test]
fn router_vm_keeps_firmware_macs_passthrough_and_start_order() {
    let c = parse_conf(VM_ROUTER);
    let p = plan_vm(203, &c, &resolve_all, &host());
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    every_key_has_a_row(&c, &p);
    let vm = p.vm.as_ref().unwrap();
    assert_eq!(p.target_id, "edge-router");
    assert_eq!((vm.cpus, vm.memory), (2, 2048));
    assert_eq!(vm.hardware.machine, MachineType::Q35);
    assert_eq!(vm.hardware.firmware, Firmware::Ovmf);
    assert!(!vm.hardware.secure_boot, "pre-enrolled-keys=0");
    assert!(!vm.hardware.balloon && vm.hardware.guest_agent);
    assert_eq!(vm.hardware.cpu_type.as_deref(), Some("host"));
    let macs: Vec<_> = vm
        .hardware
        .nets
        .iter()
        .map(|n| (n.mac.clone().unwrap(), n.bridge.clone()))
        .collect();
    assert_eq!(
        macs,
        vec![
            ("52:54:00:10:20:30".to_string(), "vmbr0".to_string()),
            ("52:54:00:11:20:31".to_string(), "vmbr2".to_string())
        ]
    );
    assert_eq!(vm.hardware.hostpci.len(), 1);
    assert_eq!(vm.hardware.hostpci[0].device, "0000:03:00.0");
    assert!(vm.hardware.hostpci[0].managed);
    assert!(
        vm.hardware.adv.serial.is_empty(),
        "serial0 is the built-in console"
    );
    assert_eq!(
        vm.hardware.adv.smbios.as_ref().unwrap().uuid.as_deref(),
        Some("6b0d549b-6f03-475a-9600-a35a099950d8")
    );
    assert_eq!(
        vm.hardware.adv.vmgenid.as_deref(),
        Some("8d116ece-1738-47d9-bd9c-172411e20b8f")
    );
    let d = &vm.disks[0];
    assert_eq!(
        (
            d.disk_type.as_str(),
            d.ssd,
            d.iothread,
            d.discard,
            d.cache.as_str()
        ),
        ("scsi", true, true, true, "none")
    );
    assert_eq!(d.size_gb, 16);
    assert_eq!(
        p.autostart,
        Autostart {
            onboot: true,
            order: Some(1),
            up_delay_secs: None
        }
    );
    assert_eq!(p.tags, vec!["firewall", "router"]);
    assert!(p
        .volumes
        .iter()
        .any(|v| v.role == VolRole::Efivars && v.source == "local-zfs:vm-203-disk-0"));
    assert_eq!(row(&p, "snapshots").carry, Carry::NotCarried);
    assert_eq!(row(&p, "description").carry, Carry::NotCarried);
}

#[test]
fn missing_pci_device_or_bridge_blocks_the_import() {
    let c = parse_conf(VM_ROUTER);
    let mut h = host();
    h.pci.clear();
    let p = plan_vm(203, &c, &resolve_all, &h);
    assert!(
        p.blockers
            .iter()
            .any(|b| b.contains("0000:03:00.0 does not exist")),
        "{:?}",
        p.blockers
    );
    let mut h = host();
    h.pci.insert("0000:03:00.0".into(), None);
    h.bridges = vec!["vmbr0".into()];
    let p = plan_vm(203, &c, &resolve_all, &h);
    assert!(p.blockers.iter().any(|b| b.contains("no IOMMU group")));
    assert!(p
        .blockers
        .iter()
        .any(|b| b.contains("bridge vmbr2 does not exist")));
    // mapped to an existing bridge: fine
    h.bridge_map.insert("vmbr2".into(), "wan0".into());
    h.bridges.push("wan0".into());
    h.pci.insert("0000:03:00.0".into(), Some(1));
    let p = plan_vm(203, &c, &resolve_all, &h);
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert_eq!(p.vm.unwrap().hardware.nets[1].bridge, "wan0");
}

#[test]
fn dns_vm_carries_disk_limits_and_the_cloud_init_seed() {
    let c = parse_conf(VM_DNS);
    let p = plan_vm(202, &c, &resolve_all, &host());
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    every_key_has_a_row(&c, &p);
    let vm = p.vm.as_ref().unwrap();
    let t = vm.disks[0].throttle;
    assert_eq!((t.bps_rd, t.bps_wr), (Some(100 << 20), Some(40 << 20)));
    assert!(p
        .volumes
        .iter()
        .any(|v| v.role == VolRole::Cloudinit && v.source == "local-zfs:vm-202-cloudinit"));
    assert_eq!(row(&p, "ipconfig0").carry, Carry::Changed);
    assert!(row(&p, "sshkeys").value.starts_with("ssh-ed25519 AAAA"));
    assert_eq!(vm.display.video, VideoDevice::Std);
    assert_eq!(p.autostart.order, Some(4));
    assert!(p.description.starts_with("DNS resolver b: second"));
}

#[test]
fn legacy_vm_keeps_bios_balloon_pointer_and_boot_order() {
    let c = parse_conf(VM_LEGACY);
    let p = plan_vm(201, &c, &resolve_all, &host());
    every_key_has_a_row(&c, &p);
    let vm = p.vm.as_ref().unwrap();
    assert_eq!(vm.hardware.machine, MachineType::I440fx);
    assert_eq!(vm.hardware.firmware, Firmware::Seabios);
    assert!(vm.hardware.balloon);
    assert!(vm.display.relative_mouse, "tablet: 0");
    assert!(vm.hardware.adv.protection && p.protection);
    assert_eq!(
        vm.hardware.boot,
        vec![BootDevice::Disk, BootDevice::Cdrom, BootDevice::Net]
    );
    assert!(!p.autostart.onboot);
    assert_eq!(row(&p, "ide2").carry, Carry::Carried);
}

#[test]
fn boot_disk_becomes_disk_zero() {
    let c = parse_conf(
        "boot: order=scsi1\nscsi0: local-zfs:vm-9-disk-0,size=1G\nscsi1: local-zfs:vm-9-disk-2,size=64G\nscsihw: virtio-scsi-single\nname: x\n",
    );
    let p = plan_vm(9, &c, &resolve_all, &host());
    let vm = p.vm.unwrap();
    assert_eq!(vm.disks[0].size_gb, 64);
    let d0 = p.volumes.iter().find(|v| v.index == Some(0)).unwrap();
    assert_eq!(d0.source, "local-zfs:vm-9-disk-2");
}

#[test]
fn tpm_versions() {
    let base = "name: w\nbios: ovmf\nscsi0: local-zfs:vm-5-disk-0,size=4G\n";
    let p = plan_vm(
        5,
        &parse_conf(&format!(
            "{base}tpmstate0: local-zfs:vm-5-disk-1,size=4M,version=v2.0\n"
        )),
        &resolve_all,
        &host(),
    );
    let hw = &p.vm.as_ref().unwrap().hardware;
    assert!(hw.tpm && hw.tpm_state_expected && hw.tpm_model == Some(TpmModel::Tis));
    assert!(p.volumes.iter().any(|v| v.role == VolRole::Tpm));
    let p = plan_vm(
        5,
        &parse_conf(&format!(
            "{base}tpmstate0: local-zfs:vm-5-disk-1,size=4M,version=v1.2\n"
        )),
        &resolve_all,
        &host(),
    );
    assert!(p.blockers.iter().any(|b| b.contains("TPM v1.2")));
    let gone = |_: &PveVol| -> Result<Resolved, String> { Err("missing".into()) };
    let p = plan_vm(
        5,
        &parse_conf(&format!(
            "{base}tpmstate0: local-zfs:vm-5-disk-1,size=4M,version=v2.0\n"
        )),
        &gone,
        &host(),
    );
    assert!(p.blockers.iter().any(|b| b.contains("TPM state volume")));
}

#[test]
fn raw_args_and_windows_clock_are_reported() {
    let c = parse_conf("name: w\nostype: win11\nargs: -cpu host\nscsi0: local-zfs:vm-6-disk-0,size=4G\nlock: backup\n");
    let p = plan_vm(6, &c, &resolve_all, &host());
    assert_eq!(row(&p, "args").carry, Carry::NotCarried);
    assert!(row(&p, "localtime").note.contains("RealTimeIsUniversal"));
    assert!(p.blockers.iter().any(|b| b.contains("locked")));
}

#[test]
fn container_with_a_vpn_device_and_io_limits() {
    let c = parse_conf(CT_TUN);
    let mut h = host();
    h.io_devices
        .insert("8:16".into(), "/dev/disk/by-id/usb-Example_Disk".into());
    let p = plan_ct(211, &c, &resolve_all, &h);
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    every_key_has_a_row(&c, &p);
    let ct = p.ct.as_ref().unwrap();
    let o = ct.lxc.as_ref().unwrap();
    assert!(o.unprivileged && o.idmap_base == Some(100_000));
    assert!(o.features.nesting);
    assert_eq!(o.devices, vec!["/dev/net/tun".to_string()]);
    assert_eq!(o.io_limits.len(), 1);
    assert_eq!(o.io_limits[0].device, "/dev/disk/by-id/usb-Example_Disk");
    assert_eq!(
        o.io_limits[0].limits(),
        "rbps=31457280 wbps=20971520 riops=300 wiops=300"
    );
    assert_eq!((o.startup_order, o.onboot), (Some(10), true));
    assert_eq!(o.mounts[0].source.as_deref(), Some("/tank/backups/laptop"));
    assert_eq!(o.mounts[0].path, "/srv/backups");
    let n = &o.nets[0];
    assert_eq!(
        (n.ip.as_deref(), n.gateway.as_deref(), n.hwaddr.as_deref()),
        (
            Some("192.0.2.32/24"),
            Some("192.0.2.1"),
            Some("52:54:00:20:20:41")
        )
    );
    assert_eq!(o.disk_gb, Some(8));
    assert_eq!(ct.cpus, 1);
    assert_eq!(p.target_id, "backup-smb");
    // without a device path the limit is reported, not guessed
    let p = plan_ct(211, &c, &resolve_all, &host());
    assert!(p.ct.clone().unwrap().lxc.unwrap().io_limits.is_empty());
    assert_eq!(row(&p, "lxc.cgroup2.io.max").carry, Carry::NotCarried);
}

#[test]
fn container_rate_and_keyctl() {
    let c = parse_conf(CT_RATE);
    let p = plan_ct(212, &c, &resolve_all, &host());
    every_key_has_a_row(&c, &p);
    let o = p.ct.unwrap().lxc.unwrap();
    assert_eq!(o.nets[0].rate_mbps, Some(80), "10 MB/s");
    assert!(o.nets[0].gateway.is_none());
    assert!(o.features.keyctl && o.features.nesting);
    assert!(!o.onboot);
}

#[test]
fn container_storage_volume_becomes_a_managed_volume() {
    let c = parse_conf(CT_VOLUME);
    let p = plan_ct(213, &c, &resolve_all, &host());
    every_key_has_a_row(&c, &p);
    let o = p.ct.as_ref().unwrap().lxc.as_ref().unwrap();
    assert_eq!(o.mounts[0].volume.as_deref(), Some("mp0"));
    assert!(o.mounts[0].source.is_none());
    let v = p.volumes.iter().find(|v| v.role == VolRole::Mount).unwrap();
    assert_eq!(
        (v.index, v.source.as_str()),
        (Some(0), "local-zfs:subvol-213-disk-1")
    );
    assert_eq!(v.resolved.as_ref().unwrap().kind, VolKind::Dir);
    let m = parse_conf(CT_MIRROR);
    every_key_has_a_row(&m, &plan_ct(210, &m, &resolve_all, &host()));
}

/// The Horcrux command line of each imported VM shows the guest the same
/// machine as Proxmox did: every fact equal, or a documented difference.
#[test]
fn qemu_command_lines_match_for_carried_settings() {
    use crate::api::proxmox_import::cmdline;
    let dumps = cmdline::split_showcmd_dump(SHOWCMD);
    let ovmf = crate::vm::firmware::Ovmf {
        code: PathBuf::from("/usr/share/edk2/OVMF_CODE_4M.fd"),
        vars_template: PathBuf::from("/usr/share/edk2/OVMF_VARS_4M.fd"),
    };
    for (vmid, text) in [(201, VM_LEGACY), (202, VM_DNS), (203, VM_ROUTER)] {
        let p = plan_vm(vmid, &parse_conf(text), &resolve_all, &host());
        let vm = p.vm.unwrap();
        let hx = crate::api::proxmox_import::horcrux_argv_in(
            std::path::Path::new("/vms"),
            &vm,
            Some(&ovmf),
        );
        assert!(!hx.is_empty());
        let pve = cmdline::facts(&cmdline::shell_words(&dumps[&vmid]));
        let rows = cmdline::diff(
            &pve,
            &cmdline::facts(&hx),
            &crate::api::proxmox_import::expected_differences(),
        );
        let bad: Vec<_> = rows
            .iter()
            .filter(|r| !r.same && r.note.is_empty())
            .collect();
        assert!(
            bad.is_empty(),
            "VM {vmid}: {bad:#?}\nhorcrux: {}",
            hx.join(" ")
        );
        for must in [
            "net0",
            "smbios-uuid",
            "vmgenid",
            "machine",
            "firmware",
            "disk0",
        ] {
            assert!(
                rows.iter().any(|r| r.fact == must && r.same),
                "VM {vmid}: {must} not compared: {rows:#?}"
            );
        }
    }
}

#[test]
fn shell_words_reads_qm_showcmd() {
    let w = crate::api::proxmox_import::cmdline::shell_words(
        "/usr/bin/kvm \\\n  -id 1 \\\n  -device 'a,b=c' -x \"q \\\"r\"",
    );
    assert_eq!(
        w,
        vec![
            "/usr/bin/kvm",
            "-id",
            "1",
            "-device",
            "a,b=c",
            "-x",
            "q \"r"
        ]
    );
}

/// Real configs (not in the repository): `HX_PVE_REAL_DIR=<dir with
/// qemu-server/ and lxc/>` maps every guest without an unknown key.
#[test]
fn real_configs_from_env() {
    let Ok(dir) = std::env::var("HX_PVE_REAL_DIR") else {
        return;
    };
    let mut n = 0;
    for (sub, ct) in [("qemu-server", false), ("lxc", true)] {
        for e in std::fs::read_dir(PathBuf::from(&dir).join(sub))
            .unwrap()
            .flatten()
        {
            let text = std::fs::read_to_string(e.path()).unwrap();
            let id: u32 = e
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".conf")
                .parse()
                .unwrap();
            let c = parse_conf(&text);
            let mut h = host();
            h.bridges
                .extend(["vmbr98".to_string(), "vmbr99".to_string()]);
            let p = if ct {
                plan_ct(id, &c, &resolve_all, &h)
            } else {
                plan_vm(id, &c, &resolve_all, &h)
            };
            every_key_has_a_row(&c, &p);
            eprintln!("{id}: {} rows, blockers {:?}", p.rows.len(), p.blockers);
            n += 1;
        }
    }
    assert!(n > 0);
}
