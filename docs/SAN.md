# SAN storage: iSCSI, NVMe over Fabrics, Fibre Channel, multipath and shared LVM

Horcrux manages block storage on a SAN for the whole cluster: an iSCSI
target is configured once, every node logs in on every path the array
advertises, dm-multipath turns those paths into one device per LUN, and a
volume group on that LUN is shared by all nodes with node locking. NVMe
over Fabrics subsystems (NVMe/TCP, NVMe/RDMA) are connected the same way on
every path, with the kernel's native NVMe multipath. Fibre Channel LUNs
(zoned to a node's HBA) join the same multipath maps and path view.
Snapshots on a shared thick VG are kept as a qcow2 volume chain. The web UI
has a SAN page (Storage > SAN, per node) and an "Add SAN storage" wizard.
Parity notes: `docs/design/proxmox-9.3-parity.md`, item 4.

Build: Cargo feature / USE flag `san` (on in the default build and the
prebuilt `horcrux-bin`). On `horcrux-bin` the USE flag only pulls the host
tools: `sys-block/open-iscsi`, `sys-fs/multipath-tools`,
`sys-fs/lvm2[lvm,sanlock]`, `sys-cluster/sanlock`, `sys-apps/util-linux`,
`sys-apps/nvme-cli`.
A node without a tool answers **424** with a reason code instead of failing:

| reason | missing | needed for |
|---|---|---|
| `san-iscsiadm-missing` | `iscsiadm` (open-iscsi) | discovery, logins |
| `san-multipathd-missing` | `multipathd`, `multipath` | multipath (or turn `enabled` off) |
| `san-lvmlockd-missing` | `lvmlockd`, `sanlock` | shared volume groups |
| `san-blkdiscard-missing` | `blkdiscard` | `wipe_removed` / `discard_removed` |
| `san-nvme-cli-missing` | `nvme` (nvme-cli) | NVMe-oF discovery, connects |
| `san-nvme-transport-missing` | kernel module `nvme_tcp` / `nvme_rdma` | NVMe-oF on that transport |
| `san-fc-no-hba` | an FC HBA (nothing in `/sys/class/fc_host`) | FC rescan |
| `san-qemu-img-missing` | `qemu-img` | snapshots as a volume chain |

The background apply reports the same reasons in `GET /api/san/status`
(`last_apply.missing`).

## API

All routes need `Datastore.Audit` (read) or `Datastore.Allocate` (write)
on `/storage`.

| route | what |
|---|---|
| `GET/PUT /api/san/settings` | multipath and apply settings (one cluster-wide object) |
| `GET/POST /api/san/targets` | iSCSI targets |
| `GET/PUT/DELETE /api/san/targets/:id` | one target |
| `POST /api/san/discover` `{"portals": [...]}` | SendTargets discovery: every target and every portal it advertises (nothing is stored) |
| `GET /api/san/status` | this node: tools, sanlock host id, last apply report |
| `GET /api/san/paths` | this node: each LUN, its multipath map, its paths (device, portal, local address, state) |
| `GET /api/san/nodes/:node/paths` | the same, asked of another cluster member |
| `POST /api/san/apply` | apply now on this node and return the report |
| `GET/POST /api/san/lvm`, `GET/DELETE /api/san/lvm/:id` | shared volume groups used as storage pools on every node |
| `GET/POST /api/san/nvme/targets`, `GET/PUT/DELETE /api/san/nvme/targets/:id` | NVMe-oF subsystems |
| `POST /api/san/nvme/discover` `{"transport": "tcp", "addresses": [...]}` | discovery log of the listed discovery controllers: every subsystem and each I/O controller address (nothing is stored) |
| `GET /api/san/fc` | this node's FC HBA ports (WWPN, WWNN, state, speed, fabric) and the remote ports each sees, with the LUNs behind target ports |
| `POST /api/san/fc/rescan` `{"host": 5, "mode": "scan"}` | rescan one HBA (or all without `host`); `mode` defaults to the `fc_rescan_mode` setting |
| `GET/POST /api/storage/pools/:pool/volumes/:vol/snapshots` | snapshot chain of a volume (oldest first) / take `{"name"}` (volume-chain LVM pools) |
| `DELETE /api/storage/pools/:pool/volumes/:vol/snapshots/:snap` | delete one snapshot (its data merges into the next one) |
| `POST /api/storage/pools/:pool/volumes/:vol/snapshots/:snap/rollback` | roll back to the newest snapshot |

The declarative config has the same objects in a `san:` section
(`settings`, `targets`, `shared_lvm`, `nvme_targets`; export/plan/apply,
prune deletes targets, NVMe-oF subsystems and shared VGs the spec lacks).

### Targets

```json
{
  "id": "array1",
  "iqn": "iqn.2003-01.org.linux-iscsi.array1:sn.0123",
  "portals": ["10.10.1.1", "10.10.2.1:3260"],
  "use_advertised_portals": true,
  "chap_user": "horcrux",
  "chap_secret": "write-only, 12..256 characters",
  "replacement_timeout_secs": 5,
  "login_timeout_secs": 15,
  "startup_automatic": true,
  "nodes": []
}
```

* `portals` are where discovery starts. With `use_advertised_portals`
  (default) each node logs in on every portal the target advertises, so an
  array with 2 controllers x 2 ports gives 4 paths without listing them.
* `chap_secret` is write-only: answers show `chap_secret_set`. It is stored
  on each node in `<data_dir>/secrets/san/<id>.chap` (mode 0600) and sent to
  the other members only inside the authenticated peer sync; it is never
  part of the replicated rows, exports or logs. `""` clears it; leaving it
  out keeps the stored one. Only one-way CHAP is supported in phase 1.
* `replacement_timeout_secs` (default 5) is how long a dead session holds
  I/O before multipath fails over (`node.session.timeo.replacement_timeout`).
* `nodes` limits a target to some members (empty = all).

### NVMe over Fabrics

```json
{
  "id": "flash1",
  "subnqn": "nqn.2014-08.com.example:flash1",
  "transport": "tcp",
  "addresses": ["10.10.1.1", "10.10.2.1:8009"],
  "use_advertised_addresses": true,
  "ctrl_loss_tmo_secs": 600,
  "reconnect_delay_secs": 10,
  "iopolicy": "round-robin",
  "nodes": []
}
```

* `addresses` are discovery controllers (default port 8009). With
  `use_advertised_addresses` (default) each node runs `nvme discover` on
  every listed address and connects to every I/O controller the discovery
  logs report for `subnqn`; with it off, the listed addresses are the I/O
  controllers themselves (default port 4420).
* `ctrl_loss_tmo_secs` (kernel default 600; -1 = reconnect forever) and
  `reconnect_delay_secs` (10) are passed to `nvme connect`.
* Multipath: with native NVMe multipath (`nvme_core.multipath=Y`, the
  default on Gentoo and distribution kernels) the paths of a namespace form
  one head device (`/dev/nvme1n1`); Horcrux sets the subsystem's
  `iopolicy` (`numa`, `round-robin` (default), `queue-depth` on 6.11+).
  With native multipath off, each path is its own block device and
  dm-multipath maps them by WWID like iSCSI LUNs (when `enabled`).
* A node without `/etc/nvme/hostnqn` gets one (`nvme gen-hostnqn`) once, so
  the array sees the same host after every reboot; an existing one is never
  changed.
* In `GET /api/san/paths` an NVMe namespace is a LUN with `transport:
  "nvme"` paths: `map` is the head device, each path shows the controller,
  its address and local address, the controller state (`device_state`:
  `live`, `connecting`, ...) and the ANA state (`checker_state`:
  `optimized`, `non-optimized`, `inaccessible`). `native_nvme_multipath`
  says which mode the node runs. Only subsystems configured here are shown.
* In-band authentication (DH-HMAC-CHAP) is not configured yet.

### Multipath settings

Horcrux owns one drop-in, `config_path` (default
`/etc/multipath/conf.d/horcrux.conf`, which multipathd reads through its
default `config_dir`). `/etc/multipath.conf` is never edited, and a file at
`config_path` that Horcrux did not write is never replaced.

| key | default | multipath.conf |
|---|---|---|
| `enabled` | `true` | (off: no drop-in, LUNs used as single devices) |
| `path_grouping_policy` | `multibus` | also `failover`, `group_by_prio`, `group_by_serial`, `group_by_node_name` |
| `path_selector` | `service_time` | `service-time 0`, `round-robin 0`, `queue-length 0` |
| `no_path_retry` | `12` | `queue`, `fail` or a count of checker intervals |
| `failback` | `immediate` | `manual`, `followover` or seconds |
| `polling_interval_secs` | `5` | `polling_interval` |
| `find_multipaths` | `strict` | Horcrux registers the WWIDs of its own SAN LUNs (`multipath -a`), so local disks are never grabbed |
| `user_friendly_names` | `false` | maps are named by WWID, identical on every node |
| `fast_io_fail_tmo_secs` / `dev_loss_tmo_secs` | `5` / `600` | FC/SAS timeouts |
| `blacklist`, `blacklist_exceptions` | `[]` | `{"match":"wwid","wwid":..}`, `{"match":"devnode","devnode":..}`, `{"match":"device","vendor":..,"product":..}` |
| `reconcile_interval_secs` | `30` | how often each node re-applies (and pulls from peers) |
| `sanlock_watchdog` | `true` | `false` writes `use_watchdog = 0` to `/etc/sanlock/sanlock.conf` (test beds without a watchdog device only) |
| `fc_multipath` | `true` | register the WWIDs of LUNs behind online FC target ports on each apply (multipath maps for FC) |
| `fc_rescan_mode` | `scan` | default of `POST /api/san/fc/rescan`: `scan` or `lip` |

### Shared LVM

```json
{"id": "san-lvm", "vg_name": "vg_san", "device": "36001405d27e5d898829468b90ce4ef8c",
 "wipe_removed": false, "discard_removed": true, "snapshot_as_volume_chain": true,
 "create_vg": true}
```

`device` is the WWID of the multipath map (`/dev/mapper/<wwid>`) or a
`/dev/...` path. `create_vg: true` runs `vgcreate --shared --locktype sanlock`
on the node that receives the request; every node then joins the VG's
lockspace (`vgchange --lockstart`) and gets a storage pool named `id` of
type LVM with `lvm.shared = true`. Removing the shared LVM storage removes
the pools and leaves the lockspace; the VG and its data stay on the LUN.

Lock manager: **lvmlockd with sanlock**. sanlock keeps its leases on the
shared LUN itself, so it needs no extra cluster stack; Gentoo packages it
(`sys-cluster/sanlock`, lvm2 `USE=sanlock` builds lvmlockd with it) with
both OpenRC init scripts and systemd units, while dlm is not in the Gentoo
tree. Each node gets a sanlock `host_id` (1..2000) that Horcrux allocates
and replicates (`GET /api/san/status`), written with `use_lvmlockd = 1` to
`/etc/lvm/lvmlocal.conf`, which Horcrux owns on SAN nodes (a packaged,
comment-only file is kept as `lvmlocal.conf.horcrux-orig`; a file with
admin settings is refused). Services started and enabled through the
node's init system: `iscsid`, `multipathd`, `sanlock`, `wdmd` (with the
watchdog), `lvmlockd`.

LVs on a shared VG are activated exclusively (`lvchange -aey`) on the node
that uses them; another node's activation is refused while one holds it.

### Removed volumes: wipe and discard

Per LVM storage pool (`lvm` options on `POST /api/storage/pools`, and on
every shared VG):

* `wipe_removed`: zero the volume before `lvremove` (`blkdiscard
  --zeroout`), so the next volume on those extents never reads old guest
  data.
* `discard_removed`: discard the volume's extents (UNMAP/TRIM), returning
  the space to a thin-provisioned array. A device without discard support
  is removed anyway (logged); a failed wipe stops the removal.
* `command_timeout_secs` (60) and `scrub_timeout_secs` (3600) bound the LVM
  and scrub steps.

### Fibre Channel

FC needs no login: the HBA and the fabric zoning decide what a node sees.
Horcrux reads `/sys/class/fc_host` (HBA ports) and
`/sys/class/fc_remote_ports` (remote ports, the LUNs under each target port)
and treats every (HBA, target port) pair as one path. With the settings
key `fc_multipath` (default true) each apply registers the WWIDs of the
LUNs behind online target ports, so they get multipath maps like iSCSI
LUNs, and the path view lists them with transport `fc`, the remote port's
FC address as `portal` and the HBA's WWPN as `local_address`. A blocked or
missing remote port, or an HBA that is not `Online`, makes its path
`failed`. `POST /api/san/fc/rescan` scans the SCSI host for new LUNs
(`mode: scan`, non-disruptive) or also issues a LIP (`mode: lip`, the HBA
logs in to the fabric again; multipath carries I/O on the other HBAs).
`fast_io_fail_tmo_secs` / `dev_loss_tmo_secs` in the multipath settings
apply to FC remote ports.

QEMU emulates no FC HBA, so FC is covered by unit tests against a sysfs
fixture tree (`san/fc.rs`) and by the e2e's no-HBA checks (empty view,
424 rescan), not by a real fabric.

### Snapshots as a volume chain

`snapshot_as_volume_chain: true` on a shared LVM storage (or in the `lvm`
options of an LVM pool) keeps every volume as a qcow2 image written straight
onto its thick LV. A snapshot `s1` of `vm-100-disk-0`:

1. renames the current LV to `snap_vm-100-disk-0_s1` (it is now frozen),
2. creates a new `vm-100-disk-0` LV (sized by `qemu-img measure` for a
   fully allocated image, so it can always hold every cluster),
3. writes a qcow2 overlay on it whose backing file is the frozen LV.

Rollback goes to the newest snapshot only (as on Proxmox VE): the current LV
is removed (with `wipe_removed` / `discard_removed`) and a fresh overlay on
that snapshot takes its place; for an older one, delete the newer snapshots
first. Deleting a snapshot rebases its child (the next snapshot or the
current LV) onto its parent with `qemu-img rebase`, which copies the
snapshot's data into the child, then removes the snapshot LV. Deleting the
volume removes the whole chain. Snapshot LVs are hidden from the volume
list; volumes report format `qcow2`. Every LV of a chain is activated
exclusively on the node that works on it. A volume that is a VM's disk is
refused (409), like delete and resize. This needs no LVM CoW snapshots,
which shared VGs cannot use across nodes.

### Web UI

Storage > SAN (`/storage/san`, `?node=<name>` for another member) shows the
path view of one node (every LUN with its map, size and each path's state,
address and local side), the targets, the shared LVM storages and the
node's FC HBAs with a Rescan button; it refreshes every 5 s. "Add SAN
storage" (`/storage/san/new`) is a wizard: transport and portals (iSCSI,
optional CHAP) or discovery controllers (NVMe/TCP) -> the discovered
targets with every path -> target id (stored and logged in on every node)
-> the LUN -> storage id, volume group, "Snapshots as volume chain",
discard/wipe of removed volumes -> review -> finish.

## Cluster behaviour

Settings, targets, shared VGs and host ids are rows replicated to every
member through the peer channel (`/api/ha/peer/san/*`): pushed on change,
pulled by every node each `reconcile_interval_secs`, newest `rev` wins,
deletes are tombstones. Each node applies the rows itself: logs in on every
path, writes the drop-in and reloads multipathd only when it changed,
registers the LUN WWIDs, starts the lock services, joins the lockspaces and
creates the storage pools. A node that joins later pulls everything on its
first tick. A target deleted (or limited away from a node) is logged out of
on that node; an NVMe-oF subsystem deleted the same way has its
controllers disconnected (`nvme disconnect`).
