# VM TPM state

A VM's emulated TPM 2.0 (swtpm) keeps what the guest binds to it: BitLocker
keys, Windows Hello, the Microsoft Entra / Intune device identity,
TPM-bound disk encryption (systemd-cryptenroll, LUKS-TPM2). An empty TPM in
place of the old one breaks all of them; Windows reports "Device TPM
problem" (0xc0090016, NTE_BAD_KEYSET). Horcrux therefore never replaces a
TPM silently.

State lives in `<disk0>.tpm/` (`tpm2-00.permall`). The interface model is a
VM setting (`hardware.tpm_model`: `crb` or `tis`); new VMs get CRB (what
libvirt and virt-manager use for Windows 11), older VMs stay on TIS.
Changing the model keeps the state.

| Path | What happens to the TPM |
| --- | --- |
| libvirt import (system and `qemu:///session`) | state directory copied and read back byte for byte, model, version, SMBIOS UUID and MAC kept; the old state is offered for removal only after the copy is verified. A domain whose TPM state cannot be carried (not found, encrypted, TPM 1.2, host pass-through) is refused unless `accept_new_tpm` is sent |
| Proxmox `tpmstate0` volume, any swtpm state directory | `POST /api/vms/:id/tpm/import-state {source, replace?}` (source under the image library or a pool directory) |
| VMware vTPM (ESXi import) | not portable (encrypted by VMware): the import preview warns loudly; abort or accept a new TPM |
| Backup / restore | `tpm-tpm2-00.permall` and the EFI variables are archive members |
| Disk-only snapshot / rollback | the TPM state is saved with the snapshot and rolled back with the disk (a RAM snapshot carries it in its device state) |
| Offline and live migration, HA | offline: TPM files are copied and verified; live: in the migration stream; replication: ZFS sends the dataset that holds the state |
| Save state on host shutdown | the saved state is bound to the TPM and EFI files (they are part of its consistency stamps) |

A VM that has used its TPM (`hardware.tpm_state_expected`, set by the server)
refuses to start when the state is missing (HTTP 409, with the BitLocker
warning). `GET /api/vms/:id/tpm` reports `state_present`, `model`,
`blocked`; `POST /api/vms/:id/tpm/reset {"confirm_data_loss": true}` is the
explicit "create a new TPM" action (the VM must be stopped). The VM's
Hardware page shows the model, whether the state exists and the reset
action.

Test: `horcrux-api/tests/tpm_state_e2e.sh` seals a secret under a persistent
key in a real swtpm, then unseals it from the TPM of the imported, restored,
rolled-back and Proxmox-imported VM.
