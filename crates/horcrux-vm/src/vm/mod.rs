pub mod clone;
pub mod clone_progress;
pub mod cross_node_clone;
pub mod devices;
pub mod display;
pub mod firmware;
pub mod hardware;
pub mod hold;
#[cfg(feature = "incus")]
pub mod incus;
#[cfg(feature = "lxd")]
pub mod lxd;
pub mod migrate;
/// Virtual machine management module
/// Handles QEMU/KVM, LXD, and Incus virtual machine lifecycle
pub mod qemu;
pub mod replication;
pub mod savestate;
pub mod snapshot;
pub mod snapshot_quota;
pub mod snapshot_scheduler;
pub mod vgpu;

pub use qemu::{QemuManager, QemuVm};

use crate::db::Database;
use horcrux_common::{Result, VmConfig, VmHypervisor, VmStatus};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Virtual machine manager
pub struct VmManager {
    vms: Arc<RwLock<HashMap<String, QemuVm>>>,
    qemu: QemuManager,
    #[cfg(feature = "lxd")]
    lxd: lxd::LxdManager,
    #[cfg(feature = "incus")]
    incus: incus::IncusManager,
    db: Option<Arc<Database>>,
}

impl Default for VmManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Report a QEMU VM's real state: the stored status is whatever the last
/// start/stop wrote, but QEMU can exit on its own (guest poweroff, crash,
/// kill) and then the VM is stopped no matter what the database says. An
/// exited-but-unreaped QEMU (a zombie) counts as exited.
fn live_status(vm: &mut VmConfig) {
    if vm.hypervisor == VmHypervisor::Qemu
        && matches!(vm.status, VmStatus::Running | VmStatus::Paused)
        && !QemuManager::process_running(&vm.id)
    {
        vm.status = VmStatus::Stopped;
    }
}

/// `live_status` in both directions: also a QEMU that is alive while the
/// stored status says stopped (the API died between starting QEMU and
/// recording it) is running.
fn observed_status(vm: &VmConfig) -> VmStatus {
    let mut c = vm.clone();
    live_status(&mut c);
    if c.hypervisor == VmHypervisor::Qemu
        && c.status == VmStatus::Stopped
        && QemuManager::process_running(&c.id)
    {
        return VmStatus::Running;
    }
    c.status
}

impl VmManager {
    pub fn new() -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
            #[cfg(feature = "lxd")]
            lxd: lxd::LxdManager::new(),
            #[cfg(feature = "incus")]
            incus: incus::IncusManager::new(),
            db: None,
        }
    }

    /// Create VmManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
            #[cfg(feature = "lxd")]
            lxd: lxd::LxdManager::new(),
            #[cfg(feature = "incus")]
            incus: incus::IncusManager::new(),
            db: Some(db),
        }
    }

    /// Create VmManager with database support and an explicit VM storage
    /// path (honors `HORCRUX_DATA_DIR`/`HORCRUX_VM_STORAGE` instead of the
    /// hardcoded `/var/lib/horcrux/vms` default, which doesn't exist and
    /// isn't writable in unprivileged environments like CI runners).
    pub fn with_database_and_storage_path(db: Arc<Database>, vm_storage_path: PathBuf) -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::with_storage_path(vm_storage_path),
            #[cfg(feature = "lxd")]
            lxd: lxd::LxdManager::new(),
            #[cfg(feature = "incus")]
            incus: incus::IncusManager::new(),
            db: Some(db),
        }
    }

    /// List all virtual machines
    pub async fn list_vms(&self) -> Vec<VmConfig> {
        // Try database first if available
        let listed = match &self.db {
            Some(db) => db.list_vms().await.ok(),
            None => None,
        };
        let listed = match listed {
            Some(v) => v,
            None => {
                let vms = self.vms.read().await;
                vms.values().map(|vm| vm.to_config()).collect()
            }
        };
        let mut out = Vec::with_capacity(listed.len());
        for vm in listed {
            out.push(self.reconcile(vm).await);
        }
        out
    }

    /// Bring a stored VM's status in line with reality (memory + database),
    /// and adopt the disk of a VM created by a release that didn't record
    /// disks in the database (before migration 021): its image is at the
    /// storage path as `<id>.qcow2`. Without this an upgraded VM has no
    /// disk, so backup, clone and snapshots refuse it.
    async fn reconcile(&self, mut vm: VmConfig) -> VmConfig {
        let mut changed = false;
        if vm.hypervisor == VmHypervisor::Qemu && vm.disks.is_empty() {
            let legacy = self.qemu.vm_from_config(&vm);
            if legacy.disk_path.is_file() {
                tracing::warn!(
                    "VM {}: no disk recorded; adopting {} (created by an older release)",
                    vm.id,
                    legacy.disk_path.display()
                );
                vm.disks = legacy.to_config().disks;
                changed = true;
            }
        }
        let stored = vm.status.clone();
        let live = observed_status(&vm);
        if live != stored {
            vm.status = live.clone();
            tracing::warn!(
                "VM {}: stored status {:?} but QEMU process says {:?}; correcting",
                vm.id,
                stored,
                live
            );
            if let Some(m) = self.vms.write().await.get_mut(&vm.id) {
                m.status = live;
            }
            changed = true;
        }
        if changed {
            if let Some(db) = &self.db {
                if let Err(e) = db.update_vm(&vm).await {
                    tracing::error!("VM {}: could not persist reconciled config: {e}", vm.id);
                }
            }
        }
        vm
    }

    /// Get a specific VM by ID
    pub async fn get_vm(&self, id: &str) -> Result<VmConfig> {
        // Try database first if available
        if let Some(db) = &self.db {
            if let Ok(vm) = db.get_vm(id).await {
                return Ok(self.reconcile(vm).await);
            }
        }

        // Fallback to in-memory
        let vm = {
            let vms = self.vms.read().await;
            vms.get(id)
                .map(|vm| vm.to_config())
                .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?
        };
        Ok(self.reconcile(vm).await)
    }

    /// Create a new virtual machine
    pub async fn create_vm(&self, config: VmConfig) -> Result<VmConfig> {
        self.create_vm_from_image(config, None).await
    }

    /// Create a VM; `image` is the instance image of an LXD/Incus VM
    /// (`distro/release`, `remote:alias` or a local alias). QEMU VMs take
    /// a disk image through `import-disk` instead.
    pub async fn create_vm_from_image(
        &self,
        config: VmConfig,
        image: Option<String>,
    ) -> Result<VmConfig> {
        let image = image
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty());
        if image.is_some() && !matches!(config.hypervisor, VmHypervisor::Lxd | VmHypervisor::Incus)
        {
            return Err(horcrux_common::Error::Validation(
                "image is for LXD/Incus VMs; give a QEMU VM a disk image with \
                 POST /api/vms/<id>/import-disk"
                    .to_string(),
            ));
        }
        // The ID names the disk files (`<storage>/<id>.qcow2`) and the
        // pidfile: `../../etc/x` would write a disk image outside the
        // storage directory.
        crate::validation::validate_vm_id(&config.id)?;
        let mut vms = self.vms.write().await;

        // Check if VM with this ID already exists
        if vms.contains_key(&config.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM with ID {} already exists",
                config.id
            )));
        }

        config
            .display
            .validate(&config.architecture)
            .map_err(horcrux_common::Error::Validation)?;
        // NICs (MACs generated here), CD-ROM, boot order.
        let mut config = config;
        hardware::prepare_hardware(&mut config)?;

        // Create the VM
        // Dispatch on the requested hypervisor. The LXD/Incus VM backends
        // existed but were never called: an "lxd" or "incus" VM silently
        // became a QEMU VM labelled "qemu".
        // A hypervisor whose USE flag was off at build time is refused
        // (its backend module is not compiled into this binary).
        crate::features::require_vm_hypervisor(&config.hypervisor)?;
        // Evaluate to a Result (instead of `return` in the fallback arm) so
        // a build without any hypervisor feature has no unreachable code.
        let created: Result<qemu::QemuVm> = match config.hypervisor {
            #[cfg(feature = "qemu")]
            VmHypervisor::Qemu => self.qemu.create_vm(&config).await,
            #[cfg(feature = "lxd")]
            VmHypervisor::Lxd => {
                if !lxd::LxdManager::check_lxd_available() {
                    Err(horcrux_common::Error::Validation(
                        "LXD is not installed on this host (USE=lxd needs app-containers/lxd)"
                            .to_string(),
                    ))
                } else if let Err(e) = crate::console::lxd::validate_instance_create(
                    &config.name,
                    image.as_deref().unwrap_or(""),
                ) {
                    Err(e)
                } else {
                    self.lxd.create_vm(&config, image.as_deref()).await
                }
            }
            #[cfg(feature = "incus")]
            VmHypervisor::Incus => {
                if !incus::IncusManager::check_incus_available() {
                    Err(horcrux_common::Error::Validation(
                        "Incus is not installed on this host (USE=incus needs app-containers/incus)"
                            .to_string(),
                    ))
                } else if let Err(e) = crate::console::lxd::validate_instance_create(
                    &config.name,
                    image.as_deref().unwrap_or(""),
                ) {
                    Err(e)
                } else {
                    self.incus.create_vm(&config, image.as_deref()).await
                }
            }
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::hypervisor_not_compiled(&config.hypervisor)),
        };
        let vm = created?;
        let vm_config = vm.to_config();

        // Save to database if available
        if let Some(db) = &self.db {
            db.create_vm(&vm_config).await?;
        }

        vms.insert(config.id.clone(), vm);
        Ok(vm_config)
    }

    /// Change a stopped VM's display protocol / video card / sound card.
    /// Takes effect at the next start.
    pub async fn set_display(
        &self,
        id: &str,
        display: horcrux_common::VmDisplay,
    ) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(horcrux_common::Error::Validation(
                "Display settings apply to QEMU VMs; LXD/Incus VMs always have a SPICE display \
                 and a serial console (both on the VM's Console page)"
                    .into(),
            ));
        }
        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::Conflict(
                "Stop the VM before changing its display settings".into(),
            ));
        }
        display
            .validate(&vm.architecture)
            .map_err(horcrux_common::Error::Validation)?;
        vm.display = display;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Register a VM whose disk already exists (e.g. cloned from a
    /// template) without creating a new disk.
    pub async fn import_vm(&self, config: VmConfig) -> Result<VmConfig> {
        // Template deploys, backup restores and incoming migrations all
        // register through here: the ID names the pidfile and disk files,
        // so it follows Create VM's rule.
        crate::validation::validate_vm_id(&config.id)?;
        let mut vms = self.vms.write().await;
        let known_in_db = match &self.db {
            Some(db) => db.get_vm(&config.id).await.is_ok(),
            None => false,
        };
        if vms.contains_key(&config.id) || known_in_db {
            return Err(horcrux_common::Error::Conflict(format!(
                "VM with ID {} already exists",
                config.id
            )));
        }
        // Clones arrive without MACs: give them their own.
        let mut config = config;
        hardware::prepare_hardware(&mut config)?;
        let vm = self.qemu.vm_from_config(&config);
        let vm_config = vm.to_config();
        if let Some(db) = &self.db {
            db.create_vm(&vm_config).await?;
        }
        vms.insert(config.id.clone(), vm);
        Ok(vm_config)
    }

    /// Make sure `id` is in the in-memory map, loading it from the database
    /// if needed. VMs are persisted, but start/stop used to look only at
    /// the in-memory map, so after an API restart every listed VM failed
    /// with "VM not found" when started or stopped.
    async fn hydrate(&self, vms: &mut HashMap<String, qemu::QemuVm>, id: &str) -> Result<()> {
        if vms.contains_key(id) {
            return Ok(());
        }
        if let Some(db) = &self.db {
            if let Ok(config) = db.get_vm(id).await {
                vms.insert(id.to_string(), self.qemu.vm_from_config(&config));
                return Ok(());
            }
        }
        Err(horcrux_common::Error::VmNotFound(id.to_string()))
    }

    /// Start a virtual machine
    pub async fn start_vm(&self, id: &str) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor == VmHypervisor::Qemu {
            // The in-memory status goes stale when QEMU dies on its own.
            let live = observed_status(&vm.to_config());
            if live != vm.status {
                vm.status = live;
                if let Some(db) = &self.db {
                    db.update_vm(&vm.to_config()).await?;
                }
            }
        }

        // The stored status says "running" but its QEMU is gone (the guest
        // powered off, QEMU crashed or was killed): it is stopped, and must
        // be startable again instead of "already running" forever.
        if vm.hypervisor == VmHypervisor::Qemu
            && matches!(vm.status, VmStatus::Running | VmStatus::Paused)
            && !QemuManager::process_running(&vm.id)
        {
            vm.status = VmStatus::Stopped;
        }
        let started: Result<()> = match vm.hypervisor {
            #[cfg(feature = "qemu")]
            VmHypervisor::Qemu => self.qemu.start_vm(vm).await,
            #[cfg(feature = "lxd")]
            VmHypervisor::Lxd => self.lxd.start_vm(vm).await,
            #[cfg(feature = "incus")]
            VmHypervisor::Incus => self.incus.start_vm(vm).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::hypervisor_not_compiled(&vm.hypervisor)),
        };
        started?;
        vm.status = VmStatus::Running;
        let config = vm.to_config();

        // Persist the new status if available
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }

        Ok(config)
    }

    /// Stop a virtual machine
    pub async fn stop_vm(&self, id: &str) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor == VmHypervisor::Qemu {
            // The in-memory status goes stale when QEMU dies on its own.
            let live = observed_status(&vm.to_config());
            if live != vm.status {
                vm.status = live;
                if let Some(db) = &self.db {
                    db.update_vm(&vm.to_config()).await?;
                }
            }
        }

        let stopped: Result<()> = match vm.hypervisor {
            VmHypervisor::Qemu => self.qemu.stop_vm(vm).await,
            #[cfg(feature = "lxd")]
            VmHypervisor::Lxd => self.lxd.stop_vm(vm).await,
            #[cfg(feature = "incus")]
            VmHypervisor::Incus => self.incus.stop_vm(vm).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::hypervisor_not_compiled(&vm.hypervisor)),
        };
        stopped?;
        vm.status = VmStatus::Stopped;
        let config = vm.to_config();

        // Persist the new status if available
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }

        Ok(config)
    }

    /// QEMU VMs ("name (id)") whose disks are linked-clone overlays on
    /// `disk`, directly or further down the chain (e.g. a template's disk).
    pub async fn vms_backed_by(&self, disk: &std::path::Path) -> Vec<String> {
        let vms = self.vms.read().await;
        // No VM has an empty id: every VM is a candidate.
        self.linked_dependents(&vms, "", disk).await
    }

    /// Other QEMU VMs whose disks are qcow2 overlays on top of (directly or
    /// further down the backing chain) one of VM `id`'s disks - its linked
    /// clones - as "name (id)".
    async fn linked_dependents(
        &self,
        vms: &HashMap<String, qemu::QemuVm>,
        id: &str,
        disk_path: &std::path::Path,
    ) -> Vec<String> {
        let canon =
            |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let mut own: Vec<PathBuf> = vec![canon(disk_path)];
        let mut others: Vec<VmConfig> = vms
            .values()
            .filter(|v| v.id != id)
            .map(|v| v.to_config())
            .collect();
        if let Some(db) = &self.db {
            if let Ok(cfg) = db.get_vm(id).await {
                own.extend(
                    cfg.disks
                        .iter()
                        .map(|d| canon(std::path::Path::new(&d.path))),
                );
            }
            if let Ok(all) = db.list_vms().await {
                for v in all {
                    if v.id != id && !others.iter().any(|o| o.id == v.id) {
                        others.push(v);
                    }
                }
            }
        }
        let mut dependents = Vec::new();
        for other in others {
            if other.hypervisor != VmHypervisor::Qemu {
                continue;
            }
            let mut disks: Vec<PathBuf> =
                other.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
            if disks.is_empty() {
                disks.push(self.qemu.vm_from_config(&other).disk_path);
            }
            'disks: for disk in disks {
                if !disk.is_file() {
                    continue;
                }
                for backing in QemuManager::backing_files(&disk).await {
                    if own.contains(&canon(&backing)) {
                        dependents.push(format!("{} ({})", other.name, other.id));
                        break 'disks;
                    }
                }
            }
        }
        dependents.sort();
        dependents
    }

    /// Delete a virtual machine. A QEMU VM is refused (Conflict) while it
    /// runs or while linked clones are backed by its disk.
    pub async fn delete_vm(&self, id: &str) -> Result<()> {
        let mut vms = self.vms.write().await;
        // Load DB-only VMs too, so their disk/process are cleaned up, not
        // just the row.
        let _ = self.hydrate(&mut vms, id).await;

        if let Some(vm) = vms.get(id) {
            if vm.hypervisor == VmHypervisor::Qemu {
                // A running VM used to lose its database row while QEMU kept
                // running on a disk nobody could manage any more.
                if QemuManager::process_running(id) {
                    return Err(horcrux_common::Error::Conflict(format!(
                        "VM {id} is running. Stop it before deleting it."
                    )));
                }
                let dependents = self.linked_dependents(&vms, id, &vm.disk_path).await;
                if !dependents.is_empty() {
                    return Err(horcrux_common::Error::Conflict(format!(
                        "VM {id} can't be deleted: its disk is the base image of the linked \
                         clone(s) {}. Delete those clones first (or make full clones of them \
                         and delete the linked ones); deleting this disk would leave them \
                         unbootable.",
                        dependents.join(", ")
                    )));
                }
            }
        }

        if let Some(vm) = vms.remove(id) {
            // Best-effort: the on-disk/hypervisor resources may already be
            // gone (e.g. after a server restart with a stale in-memory
            // entry); don't let that failure prevent the DB row cleanup.
            let _ = match vm.hypervisor {
                // Stopping/deleting a QEMU VM only needs the process and disk
                // handling, so a VM left in the database by an earlier
                // USE=qemu build can still be cleaned up with USE=-qemu.
                VmHypervisor::Qemu => self.qemu.delete_vm(&vm).await,
                #[cfg(feature = "lxd")]
                VmHypervisor::Lxd => self.lxd.delete_vm(&vm).await,
                #[cfg(feature = "incus")]
                VmHypervisor::Incus => self.incus.delete_vm(&vm).await,
                #[allow(unreachable_patterns)]
                _ => Err(crate::features::hypervisor_not_compiled(&vm.hypervisor)),
            };

            if let Some(db) = &self.db {
                db.delete_vm(id).await?;
            }

            Ok(())
        } else if let Some(db) = &self.db {
            // Not tracked in memory (e.g. loaded from a previous process),
            // but may still have a row in the database. Delete it there so
            // callers don't get a false VmNotFound for a VM that still
            // exists persistently.
            db.delete_vm(id).await
        } else {
            Err(horcrux_common::Error::VmNotFound(id.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};

    fn create_test_vm_config(id: &str, name: &str) -> VmConfig {
        VmConfig {
            id: id.to_string(),
            name: name.to_string(),
            hypervisor: VmHypervisor::Qemu,
            memory: 2048,
            cpus: 2,
            disk_size: 20 * 1024 * 1024 * 1024, // 20GB
            status: VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: Vec::new(),
            display: Default::default(),
            hardware: Default::default(),
        }
    }

    #[test]
    fn stored_running_without_qemu_reads_stopped() {
        // No QEMU process (no pidfile) behind a VM stored as running or
        // paused: it is stopped. Stopped stays stopped; other hypervisors
        // report their own state and are left alone.
        let id = format!("live-status-{}", std::process::id());
        for stored in [VmStatus::Running, VmStatus::Paused] {
            let mut vm = create_test_vm_config(&id, "live");
            vm.status = stored;
            live_status(&mut vm);
            assert_eq!(vm.status, VmStatus::Stopped);
        }
        let mut vm = create_test_vm_config(&id, "live");
        live_status(&mut vm);
        assert_eq!(vm.status, VmStatus::Stopped);
        let mut lxd = create_test_vm_config(&id, "lxd");
        lxd.hypervisor = VmHypervisor::Lxd;
        lxd.status = VmStatus::Running;
        live_status(&mut lxd);
        assert_eq!(lxd.status, VmStatus::Running);
    }

    #[tokio::test]
    async fn test_vm_manager_new() {
        let manager = VmManager::new();
        let vms = manager.list_vms().await;
        assert_eq!(vms.len(), 0, "New manager should have no VMs");
    }

    #[tokio::test]
    async fn test_list_vms_empty() {
        let manager = VmManager::new();
        let vms = manager.list_vms().await;
        assert_eq!(vms.len(), 0);
    }

    /// A VM ID that is not a plain file-name component is refused before
    /// any disk is created (it names `<storage>/<id>.qcow2`).
    #[tokio::test]
    async fn create_vm_refuses_path_like_ids() {
        let manager = VmManager::new();
        for id in ["../../etc/cron.d/evil", "/tmp/evil", "a/b", "-x", ""] {
            let cfg = create_test_vm_config(id, "evil");
            match manager.create_vm(cfg).await {
                Err(horcrux_common::Error::Validation(msg)) => {
                    assert!(msg.contains("VM ID"), "{id:?}: {msg}")
                }
                other => panic!("{id:?} must be refused, got {other:?}"),
            }
        }
        assert!(manager.list_vms().await.is_empty());
    }

    /// import_vm (template deploy, backup restore, incoming migration)
    /// applies the same ID rule before registering anything.
    #[tokio::test]
    async fn import_vm_refuses_path_like_ids() {
        let manager = VmManager::new();
        for id in ["../../../tmp/evil", "/tmp/evil", "a/b", "-x", "a_b.c", ""] {
            let cfg = create_test_vm_config(id, "evil");
            match manager.import_vm(cfg).await {
                Err(horcrux_common::Error::Validation(msg)) => {
                    assert!(msg.contains("VM ID"), "{id:?}: {msg}")
                }
                other => panic!("{id:?} must be refused, got {other:?}"),
            }
        }
        assert!(manager.list_vms().await.is_empty());
    }

    /// USE=-qemu/-lxd/-incus: creating a VM on a hypervisor that was not
    /// compiled in is refused before anything touches the host.
    #[tokio::test]
    async fn create_vm_refuses_hypervisors_not_compiled_in() {
        let manager = VmManager::new();
        for (h, on) in [
            (VmHypervisor::Qemu, cfg!(feature = "qemu")),
            (VmHypervisor::Lxd, cfg!(feature = "lxd")),
            (VmHypervisor::Incus, cfg!(feature = "incus")),
        ] {
            if on {
                continue;
            }
            let mut cfg = create_test_vm_config("gated-vm", "gated");
            cfg.hypervisor = h.clone();
            match manager.create_vm(cfg).await {
                Err(horcrux_common::Error::Validation(msg)) => {
                    assert!(msg.contains("not compiled"), "{h:?}: {msg}")
                }
                other => panic!("{h:?} must be refused when not compiled in, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_get_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.get_vm("non-existent").await;
        assert!(result.is_err(), "Getting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_delete_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.delete_vm("non-existent").await;
        assert!(result.is_err(), "Deleting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_start_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.start_vm("non-existent").await;
        assert!(result.is_err(), "Starting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_stop_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.stop_vm("non-existent").await;
        assert!(result.is_err(), "Stopping non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    /// VMs are persisted, so a new manager over the same database (= an
    /// API restart) must still be able to operate on them. Requires
    /// `qemu-img` (installed in CI's Test job).
    #[cfg(feature = "qemu")]
    #[tokio::test]
    async fn vm_is_usable_after_manager_restart() {
        let dir = std::env::temp_dir().join(format!("hx-vm-restart-{}", std::process::id()));
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();

        let first = VmManager::with_database_and_storage_path(db.clone(), dir.clone());
        let mut cfg = create_test_vm_config("restart-vm", "restart");
        cfg.disk_size = 1;
        first.create_vm(cfg).await.expect("create (needs qemu-img)");
        let disk = dir.join("restart-vm.qcow2");
        assert!(disk.exists());
        drop(first);

        let second = VmManager::with_database_and_storage_path(db.clone(), dir.clone());
        if let Err(horcrux_common::Error::VmNotFound(_)) = second.stop_vm("restart-vm").await {
            panic!("a persisted VM must not be 'not found' after a restart");
        }
        second.delete_vm("restart-vm").await.unwrap();
        assert!(
            !disk.exists(),
            "delete must remove the disk of a VM loaded from the DB"
        );
        assert!(second.get_vm("restart-vm").await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[tokio::test]
    async fn vm_without_recorded_disk_adopts_its_legacy_image() {
        // Releases before migration 021 kept no disks in the database.
        let dir = std::env::temp_dir().join(format!("hx-vm-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();
        let mut cfg = create_test_vm_config("legacy-vm", "legacy");
        cfg.disks = Vec::new();
        db.create_vm(&cfg).await.unwrap();
        let m = VmManager::with_database_and_storage_path(db.clone(), dir.clone());

        // No image on disk: nothing is invented.
        assert!(m.get_vm("legacy-vm").await.unwrap().disks.is_empty());

        let img = dir.join("legacy-vm.qcow2");
        std::fs::write(&img, b"img").unwrap();
        let vm = m.get_vm("legacy-vm").await.unwrap();
        assert_eq!(vm.disks.len(), 1);
        assert_eq!(vm.disks[0].path, img.to_string_lossy());
        // ... and it is persisted, not recomputed on every read.
        assert_eq!(db.get_vm("legacy-vm").await.unwrap().disks.len(), 1);
        assert_eq!(m.list_vms().await[0].disks[0].path, img.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
