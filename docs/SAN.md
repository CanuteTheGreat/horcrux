# SAN storage: iSCSI, dm-multipath and shared LVM

Horcrux manages block storage on a SAN for the whole cluster: an iSCSI
target is configured once, every node logs in on every path the array
advertises, dm-multipath turns those paths into one device per LUN, and a
volume group on that LUN is shared by all nodes with node locking. Status:
phase 1 (iSCSI + multipath + shared LVM). NVMe over Fabrics and Fibre
Channel, the UI wizard, the per-node SAN page and snapshots as a qcow2
volume chain are the next phases (`docs/design/proxmox-9.3-parity.md`,
item 4).

Build: Cargo feature / USE flag `san` (on in the default build and the
prebuilt `horcrux-bin`). On `horcrux-bin` the USE flag only pulls the host
tools: `sys-block/open-iscsi`, `sys-fs/multipath-tools`,
`sys-fs/lvm2[lvm,sanlock]`, `sys-cluster/sanlock`, `sys-apps/util-linux`.
A node without a tool answers **424** with a reason code instead of failing:

| reason | missing | needed for |
|---|---|---|
| `san-iscsiadm-missing` | `iscsiadm` (open-iscsi) | discovery, logins |
| `san-multipathd-missing` | `multipathd`, `multipath` | multipath (or turn `enabled` off) |
| `san-lvmlockd-missing` | `lvmlockd`, `sanlock` | shared volume groups |
| `san-blkdiscard-missing` | `blkdiscard` | `wipe_removed` / `discard_removed` |

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

The declarative config has the same objects in a `san:` section
(`settings`, `targets`, `shared_lvm`; export/plan/apply, prune deletes
targets and shared VGs the spec lacks).

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

### Shared LVM

```json
{"id": "san-lvm", "vg_name": "vg_san", "device": "36001405d27e5d898829468b90ce4ef8c",
 "wipe_removed": false, "discard_removed": true, "create_vg": true}
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

## Cluster behaviour

Settings, targets, shared VGs and host ids are rows replicated to every
member through the peer channel (`/api/ha/peer/san/*`): pushed on change,
pulled by every node each `reconcile_interval_secs`, newest `rev` wins,
deletes are tombstones. Each node applies the rows itself: logs in on every
path, writes the drop-in and reloads multipathd only when it changed,
registers the LUN WWIDs, starts the lock services, joins the lockspaces and
creates the storage pools. A node that joins later pulls everything on its
first tick. A target deleted (or limited away from a node) is logged out of
on that node.
