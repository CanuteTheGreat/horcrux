# VM migration between nodes

Horcrux moves its own QEMU VMs between the nodes of a cluster, from the UI
(**HA > Migration**: Migrate on a VM card, or Bulk Migration) or the API
(`POST /api/migration/jobs`, `POST /api/migrate/:vm_id`).

| Mode | When | What happens |
|---|---|---|
| **Live** (`live` / `online`) | VM running | The target starts QEMU waiting for the VM (`-incoming defer`, paused). Each disk that is not on storage both nodes share is mirrored into the target's NBD export (`drive-mirror`); once the mirrors caught up, `migrate` moves RAM and device state (TPM state included). When it completes, the mirrors are completed, the guest resumes on the target and the source QEMU ends. The guest keeps running: no reboot. |
| **Offline** | VM stopped, or chosen for a running VM | A running VM is shut down (ACPI, stopped after 60 s). Its disks (a linked clone's disk flattened on the way), EFI variable store, cloud-init seed and TPM state are copied to the target in chunks over HTTPS (all-zero chunks skipped, so sparse images stay sparse) and verified by SHA-256. The VM starts on the target if it was running. |

Both modes move the VM's firewall rules with it, keep its id, name, MACs and
hardware, and remove it (and its copied files) from the source afterwards.
Disks on **shared storage** (the same file is visible to both nodes at the
same path, e.g. NFS or CephFS mounted at the same place; detected with a
probe file per disk) are not copied: the target uses the same file.

**Cancel** (Migration Center, `DELETE /api/migration/jobs/:id`) stops the job
at once: the QEMU migration and disk mirrors are cancelled, the target
discards everything it received, and the VM keeps running on the source (in
the same QEMU process, for a live migration). A failure (e.g. the target has
no bridge of the VM's network card, or the VM's ISO isn't in its ISO
library) rolls back the same way and the job shows the reason.

## Requirements

- Both nodes are registered with each other (**HA > Cluster**, or
  `POST /api/cluster/nodes/<name> {"ip": "<address>"}`) and run with the same
  `JWT_SECRET`: the source authenticates to the target with a short-lived
  token that is only valid for the migration peer API
  (`/api/migration/peer/*`); user logins can't call it.
- The target can run the VM: its bridges exist (VLAN-aware if tagged), an ISO
  in the CD-ROM is in the target's ISO library at the same path (or eject it),
  OVMF/swtpm are installed if the VM uses UEFI/TPM, and no VM with the same
  id exists there.
- Live migration needs the same QEMU machine type on both nodes (the same
  QEMU version is safest) and a CPU type both hosts provide (`host` only
  between identical CPUs; `x86-64-v2-AES` is portable).
- Network: the target's API port, plus for live migration two ephemeral TCP
  ports per migration (NBD and the migration stream) on the address the source
  reached the target on, open only while that migration runs. That traffic is
  not encrypted: keep migrations on a trusted cluster network.

## Tests

`horcrux-api/tests/vm_migrate_e2e.sh` (CI workflow *VM migration + hardware*,
on the KVM runner) runs two nodes with real QEMU/OVMF/swtpm and CirrOS guests:
live migration of a UEFI + Q35 + TPM VM with two disks, offline migration
back, cancel, a refusing target, and live migration on shared storage.
