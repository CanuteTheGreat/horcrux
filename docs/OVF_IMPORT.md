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

## Importing straight from an ESXi host

**Virtual Machines > Import from ESXi** (`/vms/import-esxi`) lists the VMs of a standalone ESXi
host, previews one and copies it over HTTPS (the host's datastore browser, `https://<host>/folder/...`,
HTTP basic auth with an ESXi account such as `root`), then converts the disks and creates the VM
with the same mapping, safety checks and rollback as the OVF importer.

```http
POST /api/vms/import/esxi/list      {"host": "esxi1.lab", "username": "root", "password": "...", "verify_tls": false}
POST /api/vms/import/esxi/preview   {..., "datastore": "datastore1", "folder": "web01"}
POST /api/vms/import/esxi           {..., "datastore": "datastore1", "folder": "web01", "id": "web1", "bridge": "vmbr0", "storage": "<pool id>"}
```

- The VM must be **powered off** (a `.lck` file in its folder makes the import answer 409) and have
  **no snapshots** (a delta disk, `parentFileNameHint`, or a non-flat/thin disk type answers 409 and
  names the disk: delete or consolidate the snapshots in ESXi first). Only `.vmdk` disks in the VM's
  own folder are imported; RDMs and disks elsewhere are refused.
- The `.vmx` is mapped like an OVF: `numvcpus`, `memsize`, `firmware = "efi"`, SCSI/SATA/IDE/NVMe
  disk buses, `ethernetN.virtualDev` (e1000/e1000e stay `e1000`, others `virtio`, Windows `e1000`),
  CD-ROM / sound / USB reported as warnings.
- Certificates: ESXi ships a self-signed one, so verification is off unless you tick **Verify
  certificate**. The credentials are used for the request only (not stored, not logged, not echoed).
- The node itself, loopback and link-local / cloud-metadata addresses are refused (403), like URL
  imports. A VM on vCenter: export it as OVA and use the OVF importer, or point the importer at the ESXi
  host that runs it. `host` may carry a port (`esxi1.lab:8443`).
- Disk space for a copy of the VM's disks is needed in the data directory during the import
  (the staging copy is deleted disk by disk as it is converted).

## Not covered

Multi-file `.vmdk` (split extents) and `.ovf` with a manifest check (`.mf` is ignored),
vApp / multi-VM collections beyond the first VM, and OVF properties (guest customisation
`ovf-env`), live (powered-on) import from ESXi, vCenter inventories.
