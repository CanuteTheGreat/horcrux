//! Advanced VM hardware (`VmHardware::adv`): extra serial ports, virtiofs
//! shares (one `virtiofsd` per share), ivshmem, SMBIOS strings, VM Generation
//! ID, NUMA nodes, CPU/memory hotplug, the protection flag, the manual lock
//! and the hookscript that runs around the VM's lifecycle.
use super::display::{qmp_command, DisplayRuntime};
use super::qemu::QemuVm;
use super::{QemuManager, VmManager};
use horcrux_common::{Error, Result, VmAdvanced, VmConfig, VmHypervisor};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

/// Memory hotplug slots (`-m ...,slots=`).
pub const MEM_SLOTS: u32 = 8;
const HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// Host end of extra serial port / virtiofsd socket `i`.
pub fn sock(disk: &Path, kind: &str, i: usize) -> PathBuf {
    disk.with_extension(format!("{kind}{i}.sock"))
}

fn virtiofsd_pidfile(disk: &Path, i: usize) -> PathBuf {
    disk.with_extension(format!("virtiofsd{i}.pid"))
}

/// Backing file of an ivshmem device (shared by VMs with the same name).
pub fn shm_path(name: &str) -> PathBuf {
    PathBuf::from(format!("/dev/shm/hx-ivshmem-{name}"))
}

/// `-m`, `-smp` and the NUMA memory backends.
pub fn cpu_mem_args(vm: &QemuVm) -> Vec<String> {
    let adv = &vm.hardware.adv;
    let sockets = vm.hardware.sockets.filter(|s| *s > 1);
    let total_cpus = adv.max_cpus.filter(|m| *m > vm.cpus).unwrap_or(vm.cpus);
    let mut a: Vec<String> = vec!["-m".into()];
    a.push(match adv.max_memory_mb {
        Some(max) => format!("{},slots={MEM_SLOTS},maxmem={max}M", vm.memory),
        None => vm.memory.to_string(),
    });
    a.push("-smp".into());
    a.push(match sockets {
        Some(s) if total_cpus.is_multiple_of(s) => {
            let base = if total_cpus > vm.cpus {
                format!("{},maxcpus={total_cpus}", vm.cpus)
            } else {
                vm.cpus.to_string()
            };
            format!("{base},sockets={s},cores={},threads=1", total_cpus / s)
        }
        _ if total_cpus > vm.cpus => format!("{},maxcpus={total_cpus}", vm.cpus),
        _ => vm.cpus.to_string(),
    });
    let shared = !adv.virtiofs.is_empty();
    if adv.numa || shared {
        let nodes = if adv.numa {
            u64::from(
                sockets
                    .filter(|s| total_cpus.is_multiple_of(*s))
                    .unwrap_or(1),
            )
        } else {
            1
        };
        let backend = if shared {
            "memory-backend-memfd,share=on"
        } else {
            "memory-backend-ram"
        };
        let per_cpus = u64::from(total_cpus) / nodes;
        for i in 0..nodes {
            let size = vm.memory / nodes + if i == 0 { vm.memory % nodes } else { 0 };
            a.extend([
                "-object".into(),
                format!("{backend},id=hxnode{i},size={size}M"),
                "-numa".into(),
                format!(
                    "node,nodeid={i},cpus={}-{},memdev=hxnode{i}",
                    i * per_cpus,
                    (i + 1) * per_cpus - 1
                ),
            ]);
        }
    }
    a
}

fn smbios_value(v: &str) -> String {
    v.replace(',', ",,")
}

/// Serial ports, virtiofs, ivshmem, SMBIOS and VM Generation ID.
pub fn device_args(vm: &QemuVm) -> Vec<String> {
    let adv = &vm.hardware.adv;
    let mut a: Vec<String> = Vec::new();
    for (i, p) in adv.serial.iter().enumerate() {
        a.extend([
            "-chardev".into(),
            match &p.device {
                Some(dev) => format!("serial,id=hxser{i},path={dev}"),
                None => format!(
                    "socket,id=hxser{i},path={},server=on,wait=off",
                    sock(&vm.disk_path, "serial", i + 1).display()
                ),
            },
            "-device".into(),
            format!("pci-serial,chardev=hxser{i},id=hxserdev{i}"),
        ]);
    }
    for (i, v) in adv.virtiofs.iter().enumerate() {
        a.extend([
            "-chardev".into(),
            format!(
                "socket,id=hxfs{i},path={}",
                sock(&vm.disk_path, "virtiofs", i).display()
            ),
            "-device".into(),
            format!("vhost-user-fs-pci,chardev=hxfs{i},tag={}", v.tag),
        ]);
    }
    if let Some(s) = &adv.ivshmem {
        a.extend([
            "-object".into(),
            format!(
                "memory-backend-file,id=hxshm,size={}M,mem-path={},share=on",
                s.size_mb,
                shm_path(&s.name).display()
            ),
            "-device".into(),
            "ivshmem-plain,memdev=hxshm,id=hxivshmem".into(),
        ]);
    }
    if let Some(sm) = &adv.smbios {
        let mut fields = vec!["type=1".to_string()];
        for (k, v) in [
            ("manufacturer", &sm.manufacturer),
            ("product", &sm.product),
            ("version", &sm.version),
            ("serial", &sm.serial),
            ("family", &sm.family),
        ] {
            if let Some(v) = v {
                fields.push(format!("{k}={}", smbios_value(v)));
            }
        }
        if fields.len() > 1 {
            a.extend(["-smbios".into(), fields.join(",")]);
        }
    }
    if let Some(g) = &adv.vmgenid {
        let guid = if g == "auto" { new_guid() } else { g.clone() };
        a.extend(["-device".into(), format!("vmgenid,guid={guid}")]);
    }
    a
}

/// A random version-4 GUID.
pub fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Resolve `vmgenid: auto` into a stored GUID and check what needs the VM's
/// memory size.
pub fn prepare(config: &mut VmConfig) -> Result<()> {
    let adv = &mut config.hardware.adv;
    if adv.vmgenid.as_deref() == Some("auto") {
        adv.vmgenid = Some(new_guid());
    }
    if let Some(max) = adv.max_memory_mb {
        if max <= config.memory {
            return Err(Error::Validation(format!(
                "max memory ({max} MB) must be above the VM's memory ({} MB)",
                config.memory
            )));
        }
    }
    if adv.numa {
        let nodes = u64::from(config.hardware.sockets.unwrap_or(1));
        if config.memory < nodes * 64 {
            return Err(Error::Validation(format!(
                "{nodes} NUMA nodes need at least {} MB of memory",
                nodes * 64
            )));
        }
    }
    Ok(())
}

pub fn virtiofsd_binary() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(
        ["/usr/libexec", "/usr/lib/qemu", "/usr/lib/virtiofsd"]
            .iter()
            .map(PathBuf::from),
    );
    dirs.into_iter()
        .map(|d| d.join("virtiofsd"))
        .find(|p| p.is_file())
}

/// The Rust virtiofsd takes `--shared-dir`; QEMU's C one `-o source=`.
async fn rust_virtiofsd(bin: &Path) -> bool {
    tokio::process::Command::new(bin)
        .arg("--help")
        .output()
        .await
        .map(|o| {
            let t = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            t.contains("--shared-dir")
        })
        .unwrap_or(false)
}

/// Start one `virtiofsd` per share before QEMU connects to its socket.
pub async fn start_virtiofsd(vm: &QemuVm) -> Result<()> {
    let shares = &vm.hardware.adv.virtiofs;
    if shares.is_empty() {
        return Ok(());
    }
    let bin = virtiofsd_binary().ok_or_else(|| {
        Error::Validation(
            "The VM shares directories with virtiofs, but virtiofsd is not installed on this \
             host (Debian/Ubuntu: virtiofsd; Gentoo: app-emulation/virtiofsd)"
                .into(),
        )
    })?;
    for (i, v) in shares.iter().enumerate() {
        if !Path::new(&v.path).is_dir() {
            stop_virtiofsd(vm).await;
            return Err(Error::Validation(format!(
                "virtiofs share {}: {} is not a directory on this host",
                v.tag, v.path
            )));
        }
        let socket = sock(&vm.disk_path, "virtiofs", i);
        let _ = tokio::fs::remove_file(&socket).await;
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.arg(format!("--socket-path={}", socket.display()));
        if rust_virtiofsd(&bin).await {
            cmd.arg(format!("--shared-dir={}", v.path)).args([
                "--sandbox",
                "none",
                "--cache",
                "auto",
            ]);
        } else {
            // QEMU's own (C) virtiofsd
            cmd.args(["-o", &format!("source={}", v.path)]).args([
                "-o",
                "cache=auto",
                "-o",
                "sandbox=chroot",
            ]);
        }
        let child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::System(format!("virtiofsd: {e}")))?;
        if let Some(pid) = child.id() {
            let _ = tokio::fs::write(virtiofsd_pidfile(&vm.disk_path, i), pid.to_string()).await;
        }
        // reaped when it exits (QEMU stopping closes its connection)
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });
        let mut up = false;
        for _ in 0..50 {
            if socket.exists() {
                up = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !up {
            stop_virtiofsd(vm).await;
            return Err(Error::System(format!(
                "virtiofsd for {} did not create its socket {}",
                v.tag,
                socket.display()
            )));
        }
    }
    Ok(())
}

/// Stop the VM's virtiofsd processes and remove their sockets.
pub async fn stop_virtiofsd(vm: &QemuVm) {
    for i in 0..vm.hardware.adv.virtiofs.len() {
        let pf = virtiofsd_pidfile(&vm.disk_path, i);
        if let Some(pid) = tokio::fs::read_to_string(&pf)
            .await
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
        {
            let is_it = std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|c| String::from_utf8_lossy(&c).contains("virtiofsd"))
                .unwrap_or(false);
            if is_it {
                use nix::sys::signal::{kill, Signal};
                let _ = kill(nix::unistd::Pid::from_raw(pid), Signal::SIGTERM);
            }
        }
        let _ = tokio::fs::remove_file(&pf).await;
        let _ = tokio::fs::remove_file(sock(&vm.disk_path, "virtiofs", i)).await;
    }
    for i in 0..vm.hardware.adv.serial.len() {
        let _ = tokio::fs::remove_file(sock(&vm.disk_path, "serial", i + 1)).await;
    }
}

/// Run the VM's hookscript for `phase` (`pre-start`, `post-start`,
/// `pre-stop`, `post-stop`): `<script> <vmid> <phase>` with HORCRUX_VMID and
/// HORCRUX_PHASE in the environment. A failure is an error; callers decide
/// whether it aborts (pre-start does).
pub async fn run_hook(adv: &VmAdvanced, vm_id: &str, phase: &str) -> Result<()> {
    let Some(script) = &adv.hookscript else {
        return Ok(());
    };
    let out = tokio::time::timeout(
        HOOK_TIMEOUT,
        tokio::process::Command::new(script)
            .args([vm_id, phase])
            .env("HORCRUX_VMID", vm_id)
            .env("HORCRUX_PHASE", phase)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::System(format!("hookscript {script} {phase}: timed out")))?
    .map_err(|e| Error::System(format!("hookscript {script}: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Error::System(format!(
            "hookscript {script} {phase} exited with {}: {}",
            out.status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "a signal".into()),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Hook failures after the fact only warn.
pub async fn run_hook_lenient(adv: &VmAdvanced, vm_id: &str, phase: &str) {
    if let Err(e) = run_hook(adv, vm_id, phase).await {
        warn!("VM {vm_id}: {e}");
    }
}

/// What happens once QEMU is gone (any way it exited).
pub async fn after_exit(vm: &QemuVm) {
    stop_virtiofsd(vm).await;
    run_hook_lenient(&vm.hardware.adv, &vm.id, "post-stop").await;
}

/// `manual` lock: refuse the action named in the message.
pub fn check_unlocked(vm_id: &str, adv: &VmAdvanced, action: &str) -> Result<()> {
    match &adv.lock {
        Some(l) => Err(Error::Conflict(format!(
            "VM {vm_id} is locked ({l}): unlock it to {action}"
        ))),
        None => Ok(()),
    }
}

impl VmManager {
    async fn running_qemu(&self, id: &str) -> Result<(QemuVm, PathBuf)> {
        let vm = self.qemu_vm(id).await?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(Error::Validation("hotplug is for QEMU VMs".into()));
        }
        if !QemuManager::process_running(&vm.id) {
            return Err(Error::Conflict(
                "the VM is not running (change CPUs and memory in its hardware instead)".into(),
            ));
        }
        let sock = DisplayRuntime::qmp_socket(&vm.disk_path);
        Ok((vm, sock))
    }

    async fn persist_resources(&self, id: &str, cpus: u32, memory: u64) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| Error::VmNotFound(id.to_string()))?;
        vm.cpus = cpus;
        vm.memory = memory;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Add CPUs to a running VM up to `target` (needs `max_cpus` in its
    /// hardware at the last start). The new count is stored.
    pub async fn hotplug_cpus(&self, id: &str, target: u32) -> Result<VmConfig> {
        let (vm, sock) = self.running_qemu(id).await?;
        let Some(max) = vm.hardware.adv.max_cpus else {
            return Err(Error::Validation(
                "CPU hotplug is off: set \"max CPUs\" in the VM's hardware and restart it".into(),
            ));
        };
        if target <= vm.cpus {
            return Err(Error::Validation(format!(
                "the VM has {} CPUs; hotplug only adds (restart to remove)",
                vm.cpus
            )));
        }
        if target > max {
            return Err(Error::Validation(format!("at most {max} CPUs (max CPUs)")));
        }
        let r = qmp_command(&sock, json!({"execute": "query-hotpluggable-cpus"})).await?;
        let free: Vec<&serde_json::Value> = r
            .as_array()
            .map(|a| a.iter().filter(|c| c.get("qom-path").is_none()).collect())
            .unwrap_or_default();
        let need = (target - vm.cpus) as usize;
        if free.len() < need {
            return Err(Error::Conflict(format!(
                "only {} CPU slot(s) left; QEMU has no room for {need} more",
                free.len()
            )));
        }
        for (n, slot) in free.into_iter().take(need).enumerate() {
            let mut args = slot["props"].as_object().cloned().unwrap_or_default();
            args.insert("driver".into(), slot["type"].clone());
            args.insert("id".into(), json!(format!("hxcpu{}", vm.cpus as usize + n)));
            qmp_command(&sock, json!({"execute": "device_add", "arguments": args})).await?;
        }
        self.persist_resources(id, target, vm.memory).await
    }

    /// Add memory (a DIMM) to a running VM. The new size is stored.
    pub async fn hotplug_memory(&self, id: &str, add_mb: u64) -> Result<VmConfig> {
        let (vm, sock) = self.running_qemu(id).await?;
        let Some(max) = vm.hardware.adv.max_memory_mb else {
            return Err(Error::Validation(
                "memory hotplug is off: set \"max memory\" in the VM's hardware and restart it"
                    .into(),
            ));
        };
        if add_mb < 128 || !add_mb.is_multiple_of(128) {
            return Err(Error::Validation(
                "memory is added in multiples of 128 MB".into(),
            ));
        }
        if vm.memory + add_mb > max {
            return Err(Error::Validation(format!(
                "{} MB + {add_mb} MB is above the maximum of {max} MB",
                vm.memory
            )));
        }
        let devs = qmp_command(&sock, json!({"execute": "query-memory-devices"})).await?;
        let n = devs.as_array().map(|a| a.len()).unwrap_or(0);
        if n as u32 >= MEM_SLOTS {
            return Err(Error::Conflict(format!(
                "all {MEM_SLOTS} memory slots are in use; restart the VM to merge them"
            )));
        }
        let backend = if vm.hardware.adv.virtiofs.is_empty() {
            "memory-backend-ram"
        } else {
            "memory-backend-memfd"
        };
        let mut obj =
            json!({"qom-type": backend, "id": format!("hxmem{n}"), "size": add_mb * 1024 * 1024});
        if backend == "memory-backend-memfd" {
            obj["share"] = json!(true);
        }
        qmp_command(&sock, json!({"execute": "object-add", "arguments": obj})).await?;
        let mut dimm =
            json!({"driver": "pc-dimm", "id": format!("hxdimm{n}"), "memdev": format!("hxmem{n}")});
        if vm.hardware.adv.numa {
            dimm["node"] = json!(0);
        }
        if let Err(e) =
            qmp_command(&sock, json!({"execute": "device_add", "arguments": dimm})).await
        {
            let _ = qmp_command(
                &sock,
                json!({"execute": "object-del", "arguments": {"id": format!("hxmem{n}")}}),
            )
            .await;
            return Err(e);
        }
        self.persist_resources(id, vm.cpus, vm.memory + add_mb)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{
        VmArchitecture, VmHardware, VmIvshmem, VmSerialPort, VmSmbios, VmVirtiofs,
    };

    fn vm(adv: VmAdvanced, sockets: Option<u32>, cpus: u32, memory: u64) -> QemuVm {
        QemuVm {
            id: "vm1".into(),
            name: "vm1".into(),
            memory,
            cpus,
            disk_path: PathBuf::from("/vms/vm1.qcow2"),
            disk_size: 1,
            status: horcrux_common::VmStatus::Stopped,
            _pid: None,
            hypervisor: VmHypervisor::Qemu,
            architecture: VmArchitecture::X86_64,
            display: Default::default(),
            hardware: VmHardware {
                sockets,
                adv,
                ..Default::default()
            },
            disks: Vec::new(),
        }
    }

    #[test]
    fn default_command_line_is_unchanged() {
        let a = cpu_mem_args(&vm(VmAdvanced::default(), None, 2, 512));
        assert_eq!(a, ["-m", "512", "-smp", "2"]);
        let a = cpu_mem_args(&vm(VmAdvanced::default(), Some(2), 4, 512));
        assert_eq!(a, ["-m", "512", "-smp", "4,sockets=2,cores=2,threads=1"]);
        assert!(device_args(&vm(VmAdvanced::default(), None, 2, 512)).is_empty());
    }

    #[test]
    fn hotplug_limits_and_numa_nodes() {
        let adv = VmAdvanced {
            numa: true,
            max_cpus: Some(8),
            max_memory_mb: Some(4096),
            ..Default::default()
        };
        let a = cpu_mem_args(&vm(adv, Some(2), 2, 1025)).join(" ");
        assert!(a.contains("-m 1025,slots=8,maxmem=4096M"), "{a}");
        assert!(
            a.contains("-smp 2,maxcpus=8,sockets=2,cores=4,threads=1"),
            "{a}"
        );
        assert!(a.contains("memory-backend-ram,id=hxnode0,size=513M"), "{a}");
        assert!(a.contains("memory-backend-ram,id=hxnode1,size=512M"), "{a}");
        assert!(
            a.contains("-numa node,nodeid=0,cpus=0-3,memdev=hxnode0"),
            "{a}"
        );
        assert!(
            a.contains("-numa node,nodeid=1,cpus=4-7,memdev=hxnode1"),
            "{a}"
        );
    }

    #[test]
    fn virtiofs_needs_shared_memory() {
        let adv = VmAdvanced {
            virtiofs: vec![VmVirtiofs {
                tag: "data".into(),
                path: "/srv/data".into(),
            }],
            ..Default::default()
        };
        let v = vm(adv, None, 1, 256);
        let a = cpu_mem_args(&v).join(" ");
        assert!(
            a.contains("memory-backend-memfd,share=on,id=hxnode0,size=256M"),
            "{a}"
        );
        let d = device_args(&v).join(" ");
        assert!(
            d.contains("socket,id=hxfs0,path=/vms/vm1.virtiofs0.sock"),
            "{d}"
        );
        assert!(
            d.contains("vhost-user-fs-pci,chardev=hxfs0,tag=data"),
            "{d}"
        );
    }

    #[test]
    fn devices_smbios_genid_serial_shm() {
        let adv = VmAdvanced {
            serial: vec![
                VmSerialPort { device: None },
                VmSerialPort {
                    device: Some("/dev/ttyUSB0".into()),
                },
            ],
            ivshmem: Some(VmIvshmem {
                name: "bus".into(),
                size_mb: 4,
            }),
            smbios: Some(VmSmbios {
                manufacturer: Some("Acme, Inc".into()),
                serial: Some("SN1".into()),
                ..Default::default()
            }),
            vmgenid: Some("6d1f1b1e-9b53-4b5e-8f0e-0123456789ab".into()),
            ..Default::default()
        };
        let d = device_args(&vm(adv, None, 1, 256)).join(" ");
        assert!(
            d.contains("-chardev socket,id=hxser0,path=/vms/vm1.serial1.sock,server=on,wait=off"),
            "{d}"
        );
        assert!(
            d.contains("-chardev serial,id=hxser1,path=/dev/ttyUSB0"),
            "{d}"
        );
        assert!(
            d.contains("-device pci-serial,chardev=hxser1,id=hxserdev1"),
            "{d}"
        );
        assert!(
            d.contains(
                "memory-backend-file,id=hxshm,size=4M,mem-path=/dev/shm/hx-ivshmem-bus,share=on"
            ),
            "{d}"
        );
        assert!(d.contains("-device ivshmem-plain,memdev=hxshm"), "{d}");
        assert!(
            d.contains("-smbios type=1,manufacturer=Acme,, Inc,serial=SN1"),
            "{d}"
        );
        assert!(
            d.contains("-device vmgenid,guid=6d1f1b1e-9b53-4b5e-8f0e-0123456789ab"),
            "{d}"
        );
    }

    #[test]
    fn auto_genid_is_resolved_and_memory_limits_checked() {
        let mut cfg = VmConfig {
            id: "a".into(),
            name: "a".into(),
            hypervisor: VmHypervisor::Qemu,
            memory: 512,
            cpus: 1,
            disk_size: 1,
            status: horcrux_common::VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: vec![],
            display: Default::default(),
            hardware: VmHardware::default(),
        };
        cfg.hardware.adv.vmgenid = Some("auto".into());
        prepare(&mut cfg).unwrap();
        assert!(horcrux_common::valid_guid(
            cfg.hardware.adv.vmgenid.as_deref().unwrap()
        ));
        cfg.hardware.adv.max_memory_mb = Some(512);
        assert!(prepare(&mut cfg).is_err());
    }

    #[test]
    fn lock_refuses() {
        let adv = VmAdvanced {
            lock: Some("manual".into()),
            ..Default::default()
        };
        assert!(check_unlocked("1", &adv, "start it")
            .unwrap_err()
            .to_string()
            .contains("locked"));
        assert!(check_unlocked("1", &VmAdvanced::default(), "start it").is_ok());
    }

    #[tokio::test]
    async fn hookscript_runs_and_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1 $2 $HORCRUX_PHASE\" >> {}\n[ \"$2\" != fail ] || {{ echo nope >&2; exit 3; }}\n", log.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let adv = VmAdvanced {
            hookscript: Some(script.to_string_lossy().to_string()),
            ..Default::default()
        };
        run_hook(&adv, "100", "pre-start").await.unwrap();
        let e = run_hook(&adv, "100", "fail").await.unwrap_err().to_string();
        assert!(e.contains("exited with") && e.contains("nope"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "100 pre-start pre-start\n100 fail fail\n"
        );
        run_hook(&VmAdvanced::default(), "100", "pre-start")
            .await
            .unwrap();
    }
}
