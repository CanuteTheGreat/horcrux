# Importing guests from Proxmox VE

Horcrux imports Proxmox VE virtual machines (qemu-server) as Horcrux VMs and
Proxmox containers (pct) as Horcrux LXC containers. The import is planned
first (read only), then run as a job that copies every guest's data into
Horcrux's own storage and checks the copy. Nothing on the source is changed.

Web UI: **VMs > Import from Proxmox**. CLI: `horcrux import proxmox plan|run|status`.
API: `POST /api/import/proxmox/plan`, `POST /api/import/proxmox`,
`POST /api/import/proxmox/status`, `POST /api/import/proxmox/jobs`
(node administrators: `Sys.Modify` on `/`).

## Sources

| type | what it reads | typical use |
|---|---|---|
| `dir` | a directory with the guest configs (`qemu-server/*.conf`, `lxc/*.conf`, a copy of `/etc/pve`, or the `.conf` files directly), optionally `storage.cfg` and saved `qm showcmd` output (`showcmd/<id>.txt`, `<id>.showcmd` or one `guests-qm-showcmd.txt` dump with `===== VM <id>` headers) | the Proxmox host is being reinstalled: its pool was `zfs send`-ed to another pool (or disk) and the configs copied next to it |
| `ssh` | a running Proxmox node, read only (`cat /etc/pve/...`, `pvesm path`, `qm showcmd`, `dd`/`tar` of the volumes) | moving guests off a node that keeps running |
| `vzdump` | one container backup archive (`vzdump-lxc-<id>-*.tar[.zst|.gz|.lzo]`) | restoring a container from its backup |
| `adopt` | a Proxmox installation on THIS host's ZFS pool (its root file system mounted read only) | the node that ran Proxmox boots Horcrux from a new root dataset in the same pool: the guests are taken over as ZFS clones, nothing is copied (see "Adopt in place" below) |

VM backups in Proxmox's VMA format (`.vma.zst`) are not read directly: restore
them on a Proxmox node and import over SSH, or unpack them with `vma extract`
into a directory with their `qemu-server.conf`.

### Where the volumes are (`storage_map`)

For a `dir` source, every Proxmox storage id the configs name is mapped to
where its volumes are now:

```json
{"type": "dir", "path": "/tank/migration/pve",
 "storage_map": {"local-zfs": {"zfs": "tank/migration/data"},
                 "local":     {"dir": "/tank/migration/vz"}}}
```

* `{"zfs": "<dataset prefix>"}`: `vm-*` zvols are read from
  `/dev/zvol/<prefix>/<volume>`, `subvol-*` datasets from their mountpoint.
* `{"dir": "<path>"}`: Proxmox's `images/<vmid>/<file>` layout, or the files
  and directories directly inside `<path>`.

Storages missing from the map are looked up in the source's `storage.cfg`
(a ZFS pool or directory storage at the same place).

### Other request fields

* `bridge_map` `{"vmbr0": "br0"}`: Proxmox bridge -> Horcrux bridge (bridges
  not listed keep their name and must exist on this host).
* `io_devices` `{"259:0": "/dev/nvme0n1"}`: the host device behind each
  `major:minor` that a container's raw `lxc.cgroup2.io.max` line names. Those
  numbers belong to the SOURCE host (they change after a reinstall), so the
  limit is carried by device path and the numbers are looked up at every
  container start. SSH sources find the device paths on the node.
* `guests` `[111, 115]`, `ids` `{"111": "dns-b"}`, `storage` (pool for VM
  disks; default the VM storage directory).
* `start_onboot` (import only): start every guest the source started at boot
  right after it is created, in the source's start order and with its start
  delay (router first). The job reports per guest how long its import took
  (`secs`) and when it was started (`started_after_secs`).

Two guests that start at boot with the same MAC address or the same
passed-through PCI device would fight over it: the first by start order
keeps starting at boot, every later one is planned (and imported) with start
at boot off, and its plan row `onboot` says which guest it shares with.

## Adopt in place (same ZFS pool, no copy)

For a node that ran Proxmox VE on ZFS and now boots Horcrux from a new root
dataset in the same pool. Each guest volume becomes `zfs clone
<volume>@<snapshot> <target>/<guest>-<key>`: an O(1) operation, so applying
takes seconds per guest whatever the disk sizes. The Proxmox datasets are
never written: booting the Proxmox root again finds every guest exactly as it
was at the snapshot, which is the rollback, and destroying the clones undoes
the adoption on the Horcrux side.

```json
{"type": "adopt", "pve_root": "/mnt/pve-root", "snapshot": "hx-cutover",
 "target_parent_dataset": "rpool/horcrux/guests"}
```

* `pve_root`: the Proxmox root file system, mounted read only. Proxmox keeps
  its guest configs in its cluster database (`var/lib/pve-cluster/config.db`;
  `/etc/pve` is only a view of it while Proxmox runs and is empty on disk),
  which is read from a temporary copy (the write-ahead log is applied). A
  directory with a copy of `/etc/pve` works too. `storage.cfg` says which
  dataset backs each ZFS storage (`storage_map` `{"local-zfs":
  "rpool/data"}` overrides it). `node` picks the node of a cluster database
  (default: the root's `etc/hostname`, or the only node).
* `snapshot`: taken once the guests are stopped, e.g. `zfs snapshot -r
  rpool/data@hx-cutover`. A volume that changed after it (`written@<snap>`
  not 0) is refused, in the plan and again at apply, so a clone never misses
  writes. Without a snapshot the request is refused unless
  `allow_no_snapshot` is set: then the Proxmox datasets themselves are
  renamed under `target_parent_dataset` and the guests write Proxmox's data
  (booting Proxmox again no longer finds them).
* `target_parent_dataset`: where the clones go, in the same pool (created
  with `mountpoint=none` if missing).
* `showcmd`: saved `qm showcmd` output (file or directory, as for `dir`) for
  the QEMU command-line diff; the plan otherwise works fully offline.

What becomes what:

* VM disks: the cloned zvols, used as raw block devices
  (`/dev/zvol/<target>/<guest>-scsi0`); nothing is created or converted.
  The VM's own files (UEFI variable store, swtpm state, cloud-init seed,
  console sockets, serial log) live in the node's sidecar directory
  (`paths.vm_sidecars`, env `HORCRUX_VM_SIDECARS`, default
  `<vm_storage>/sidecars`) at the device path mirrored below it, because a
  device has no directory of its own. The `efidisk`, `tpmstate` and
  cloud-init volumes are read from temporary clones into those files, then
  the temporary clones are dropped.
* Container rootfs: the cloned subvolume, mounted at the container's
  `<lxc path>/<ct>/rootfs`. Owners are not touched: Proxmox's unprivileged id
  map (100000/65536) is Horcrux's, so the files are already right. The
  clone gets the source's own local properties again (`acltype`, `xattr`, a
  clone inherits from its new parent otherwise) and the Proxmox `size=` as
  `refquota`. Storage-backed mount points are cloned and mounted the same
  way; bind mounts keep their host path.
* Everything else exactly as for the other sources (MACs, UUID, vmgenid,
  hostpci, disk bus/iothread/discard/ssd, throttles, serial socket, CT
  network rate, io.max by device path, start order and onboot).
* Every clone carries the ZFS property `horcrux:guest=<id>`: deleting the
  guest in Horcrux destroys its clones, and only those. A device disk without
  that property (one Horcrux did not create) is never removed with a VM.
* The plan lists every clone with the exact `zfs clone` command; the record
  of what was taken over is `<data_dir>/proxmox-import/<id>.adopt.json`
  (with the apply time).

Cutover sequence (offline plan ahead, short window):

1. Ahead of time, from Horcrux booted once, or from a copy of the configs:
   `horcrux import proxmox plan --adopt /mnt/pve-root --snapshot hx-cutover
   --target rpool/horcrux/guests --bridge vmbr0=br0 --details` (blockers,
   every row, the clones). Bridges, PCI devices and bind-mount sources must
   exist on the Horcrux side.
2. In the window: stop the guests on Proxmox, `zfs snapshot -r
   rpool/data@hx-cutover`, boot the Horcrux root dataset.
3. On Horcrux: mount the Proxmox root read only WITHOUT changing its
   properties (`mkdir -p /mnt/pve-root; mount -t zfs -o ro,zfsutil
   rpool/ROOT/pve-1 /mnt/pve-root`; its mountpoint stays `/` for Proxmox),
   then `horcrux import proxmox run --adopt
   /mnt/pve-root --snapshot hx-cutover --target rpool/horcrux/guests --guest
   117 --guest 111 ... --start-onboot`.
4. Rollback: boot the Proxmox root again (its datasets are as at the
   snapshot); on the Horcrux side `zfs destroy -r rpool/horcrux/guests`.

Host notes: a zvol clone's partitions appear on the host like the original's
(`/dev/zvol/...-part1`); keep LVM inside guests out of the host's LVM scan
(a `global_filter` excluding `/dev/zd*`), as on Proxmox.

## What is carried

The plan lists every key of each guest config as **carried**, **changed**
(carried in another form, the note says how), **not carried** (with the
reason and what to do) or **ignored** (Proxmox bookkeeping). Settings that
would change the guest's identity or lose data are blockers: the import is
refused with the reason (a TPM 1.2 state, a missing TPM volume, a PCI device
or bridge that does not exist on this host, a Proxmox lock, and for
containers a node without the LXC runtime: install `app-containers/lxc`
first, or every imported container would fail to start).

**CPU model.** Proxmox's QEMU build has CPU models that stock QEMU (what
Horcrux runs) does not, and a VM started with one fails ("unable to find CPU
model"). The importer maps Proxmox's built-in levels to `qemu64` with their
specified feature flags and shows the mapping in the plan:

| Proxmox `cpu:` | Horcrux `cpu_type` |
|---|---|
| `x86-64-v2`, `x86-64-v2-AES`, `x86-64-v3`, `x86-64-v4` | `qemu64` plus that model's feature flags |
| `host` | `host` with KVM on this node, else `max` |
| `custom-*` (Proxmox `cpu-models.conf`) | `max` (pick a model under Hardware) |
| no `cpu:` key (Proxmox's default) | `kvm64` |
| anything else (`kvm64`, `qemu64`, `EPYC-v4`, ...) | unchanged |

`flags=+aes;-pcid` overrides the corresponding `hardware.cpu_flags`
and `hidden=1` becomes `kvm=off`; `hv-vendor-id`, `phys-bits`,
`reported-model` and similar are listed in the row's note as not carried.

VMs: machine type, firmware (OVMF + its UEFI variable store, copied from the
`efidisk` when its layout matches this host's OVMF), `pre-enrolled-keys`
(Secure Boot), the swtpm state of `tpmstate0` (converted from Proxmox's
linear volume, same keys), CPU model/cores/sockets, memory, balloon, every
disk (bus, `virtio-scsi-single`, `iothread`, `discard`, `ssd`, `cache`,
`mbps_rd`/`mbps_wr`/`iops_rd`/`iops_wr` as QEMU block throttling), the boot
disk first, the cloud-init drive (copied byte for byte as the VM's NoCloud
seed), NICs (model, **MAC kept**, **PCI slot kept** (netN = root-bus slot
`0x12`+N for net0-net5, as Proxmox seats them, so the guest's PCI path, interface
name and path-derived DHCP client-id IAID do not change; net6 and up sit behind a
PCI bridge and are not pinned, the plan row says so), bridge mapping, VLAN tag,
`rate` in MB/s as Mbit/s), PCI passthrough (managed vfio-pci; the device must exist here with
an IOMMU group), USB devices by vendor:product, `serial0: socket` (the VM's
serial console), extra serial ports, video card, guest agent, tablet,
SMBIOS UUID and strings, VM generation id, NUMA, huge pages, CPU affinity,
protection, tags, and start at boot / start order / start delay (the guest
power policy). `args:`, hook scripts, Proxmox firewall rules, snapshots and
pending changes are listed as not carried.

Containers: unprivileged (same id map 100000/65536, so owners stay as the
container saw them; a privileged container stays privileged), arch, hostname,
cores, CPU limit/units, memory, swap, the rootfs (files copied into the LXC
store with owners, and file capabilities when read from a directory; its size becomes the image size),
mount points (storage-backed ones become Horcrux volumes with their data
copied; bind mounts keep their host path, which must exist here), NICs
(name, bridge mapping, MAC, IPv4/IPv6 and gateways, VLAN, MTU, rate),
features nesting/keyctl/fuse, DNS settings, start at boot and order, tags,
device pass-through (`devN:` and `lxc.mount.entry` binds of `/dev/...`, e.g.
`/dev/net/tun`) and raw `lxc.cgroup2.io.max` limits (see `io_devices`).

### The QEMU command line

For a VM whose `qm showcmd` output is known (SSH sources fetch it; directory
sources read the saved files), the plan compares what the guest sees under
Proxmox with what it will see under Horcrux, fact by fact: machine type,
firmware, TPM model, CPU model and count, memory, every NIC's model and MAC,
every disk's bus and SSD flag, disk limits, discard, I/O threads, pointer,
clock, SMBIOS UUID, VM generation id, guest agent, balloon, serial ports and
passed-through PCI devices. Expected differences are explained in the table;
anything else is a difference to look at before the first boot.

## After the import

* The guest still exists on the source: keep it stopped there (same MACs,
  same OS). A local source's volumes (files, received ZFS volumes and
  datasets) are offered for removal on the libvirt import page's
  "Imported sources" list, with sizes, opt-in; a removal checks that the
  Horcrux copy exists and that every disk copy equals its source first.
* Windows guests ran with the clock in local time under Proxmox; Horcrux
  keeps UTC: set `RealTimeIsUniversal=1` in the guest before its first boot
  here.
* A container's bind-mount sources must exist on this host before it starts.
* The source config is kept in `<data_dir>/proxmox-import/<id>.conf` (the
  description lives there too: Horcrux guests have no description field).
