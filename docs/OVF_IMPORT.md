# Importing OVF / OVA virtual machines

Horcrux imports virtual machines exported as **OVF** (descriptor + disk files) or
**OVA** (a tar of both) from VMware vSphere/Workstation/Fusion, VirtualBox, Proxmox
(`qm importovf`), XenServer and others. The disks are converted to qcow2 with
`qemu-img` (needs `qemu-utils` on the node); the descriptor is mapped to a stopped
Horcrux VM.

## In the web UI

**Virtual Machines > Import OVF / OVA** (`/vms/import-ovf`):

1. Enter the source: a file under the images directory (`<data dir>/images`) or a
   storage pool directory (upload it with **Storage Content > Disk images**, which
   accepts `.ova`/`.ovf`/`.vmdk`), or an `http(s)` URL of an `.ova`/`.ovf`.
2. **Preview** shows the VM id, CPUs, memory, firmware, every disk with its bus, the
   network cards and a list of everything that could not be mapped. Nothing is created.
3. Pick name, bridge and storage pool, **Import**. Disk conversion can take minutes per
   100 GB; the page shows when it is done and links to the VM's hardware page.

## API

```http
POST /api/vms/import/ovf/preview   {"source": "/var/lib/horcrux/images/web.ova"}
POST /api/vms/import/ovf           {"source": "...", "id": "web1", "name": "Web 1",
                                    "bridge": "vmbr0", "storage": "<pool id>"}
```

`preview` returns `{plan, id}`; `import` answers 201 with `{vm, plan, warnings}`. Needs
VM.Allocate. A failed import (unreadable archive, bad disk, disk full) removes the VM and
all temporary files; nothing half-imported is left.

## Mapping

| OVF | Horcrux |
|---|---|
| CPU count (resource 3), memory (4, with its allocation units) | `cpus`, `memory` (clamped to 1-512 CPUs / >=16 MB, with a warning) |
| IDE / SATA(AHCI) / SCSI controller of a disk | disk bus `ide` / `sata` / `scsi` (virtio-scsi) |
| Ethernet adapter `E1000`/`E1000e` | `e1000`; `VmxNet3`, `PCNet32`, others: `virtio` with a warning |
| Windows guest (OS description) | disks on `sata`, NICs `e1000` (no virtio drivers at boot), with warnings |
| `firmware=efi` (VMware `vmw:Config`) | OVMF (UEFI) |
| Disk capacity | `size_gb` (rounded up) |
| CD-ROM, floppy, USB, sound, other devices | not imported, listed as warnings |
| Several virtual systems in one OVF | the first is imported, with a warning |

OVF networks are not mapped by name: every card is attached to the chosen bridge (default:
the node's first bridge; none = no card, with a warning). Add VLANs and MACs afterwards on
the VM's Hardware page. Tick **Windows**: after import install the virtio drivers and
switch bus/NIC if you want paravirtual speed.

## Safety

The descriptor is parsed by a built-in reader that refuses DTDs (no entity expansion) and
is limited to 8 MiB / 64 nesting levels. Disk names must be plain file names; they are
looked up only inside the archive (or next to the `.ovf`). Every disk goes through the same
`qemu-img` checks as `import-disk`: no backing file, no external data file, no VMDK whose
extents are other files (so a crafted OVA cannot copy a host file into a disk). Local
sources must be under the images directory or a pool directory; URLs may not point at the
node itself or at link-local / cloud-metadata addresses.

## Not covered

Multi-file `.vmdk` (split extents) and `.ovf` with a manifest check (`.mf` is ignored),
vApp / multi-VM collections beyond the first VM, and OVF properties (guest customisation
`ovf-env`). ESXi hosts are imported through their export (OVF/OVA), not directly.
