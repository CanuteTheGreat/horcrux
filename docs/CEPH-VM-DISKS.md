# VM disks on Ceph RBD pools

A storage pool of type `ceph` (its path is the RADOS pool name) can hold VM disks.
Each disk is an RBD image named `vm-<vm id>-disk-<index>` in that pool. The disk path
stored in the VM is `rbd:<pool>/<image>`.

## What works

| Operation | Behaviour |
| --- | --- |
| Create VM / add disk | `POST /api/vms` (`disks[].storage`) and `POST /api/vms/<id>/disks` (`{size_gb, storage: <pool id>}`) create the image with `rbd create`. |
| Grow | `PUT /api/vms/<id>/disks/<n>` with a larger `size_gb` runs `rbd resize` (shrinking is refused). |
| Remove | Detach keeps the image; `?delete_file=true` runs `rbd rm`. Deleting the VM removes its images. |
| Snapshot | VM snapshots create `rbd snap` snapshots per disk; restore is `rbd snap rollback`. |
| Clone | Full clone = independent `rbd cp`; linked clone = protected snapshot plus `rbd clone` child. |
| Migrate | RBD disks are shared storage: live and offline migration copy no disk data. Side files (QMP/serial sockets, TPM, firmware vars, cloud-init) stay on a local anchor path (`<vm storage dir>/<id>.qcow2`, never created). The target node checks its Ceph client first. |

QEMU opens the disk as `rbd:<pool>/<image>:id=<client>:conf=<ceph.conf>`.

## Configuration

- `HORCRUX_CEPH_CONF`: path of the client config (default `/etc/ceph/ceph.conf`).
- `HORCRUX_CEPH_ID`: client id without `client.` (default: librbd's `admin`).
- `HORCRUX_CEPH_BIN`: alternative `ceph` binary.

## Missing client

If the node has no `rbd` CLI, no readable ceph.conf or no usable keyring, every
operation above answers HTTP 424 (`NodeLacks`) with a reason and a hint. Nothing is
created.

## Kernel and client versions (see also CEPH-KERNEL-VERSIONS.md)

VM disks use librbd through QEMU, not the kernel RBD client, so the node kernel version
does not limit them. The `ceph` and `rbd` CLIs and `qemu-block-extra` (Debian/Ubuntu) or
QEMU built with the `rbd` block driver must be present; the Horcrux Gentoo packages enable
`USE=rbd` for QEMU.

## Tests

`horcrux-api/tests/ceph_storage_e2e.sh` boots a single-node Ceph in a container and drives
the real API: create VM on the pool, add and grow a disk, write a random pattern through
QEMU's rbd driver, snapshot, overwrite, restore, full and linked clone with data compared,
and image cleanup on VM delete.
