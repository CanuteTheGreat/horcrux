# Horcrux API Documentation

## Overview

The Horcrux API provides comprehensive virtualization management capabilities including VM lifecycle, storage, networking, monitoring, security, and clustering. All endpoints (except `/api/health` and authentication endpoints) require authentication via JWT token or API key.

**Base URL**: `http://localhost:8006`

**API Version**: 1.0

**Content-Type**: `application/json`

---

## Table of Contents

1. [Authentication](#authentication)
2. [Virtual Machines](#virtual-machines)
3. [Snapshots](#snapshots)
4. [Cloning](#cloning)
5. [Cloud-Init](#cloud-init)
6. [Backups](#backups)
7. [Templates](#templates)
8. [Storage](#storage)
9. [Networking](#networking)
10. [Console Access](#console-access)
11. [Monitoring](#monitoring)
12. [Clustering](#clustering)
13. [High Availability](#high-availability)
14. [Migration](#migration)
15. [Firewall](#firewall)
16. [GPU Passthrough](#gpu-passthrough)
17. [Security](#security)
18. [Webhooks](#webhooks)
19. [Observability](#observability)
20. [Audit Logging](#audit-logging)

---

## Authentication

### Login

Authenticates a user and returns a JWT token.

```http
POST /api/auth/login
Content-Type: application/json

{
  "username": "admin",
  "password": "secure_password"
}
```

**Response**:
```json
{
  "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "user": {
    "id": 1,
    "username": "admin",
    "role": "Administrator"
  },
  "expires_at": "2025-10-10T10:30:45Z"
}
```

**Status Codes**:
- `200 OK` - Authentication successful
- `401 AUTHENTICATION_FAILED` - Invalid credentials
- `429 RATE_LIMITED` - Too many failed login attempts

### Register User

Creates a new user account.

```http
POST /api/auth/register
Content-Type: application/json

{
  "username": "newuser",
  "password": "secure_password",
  "email": "user@example.com"
}
```

**Response**:
```json
{
  "id": 2,
  "username": "newuser",
  "email": "user@example.com",
  "created_at": "2025-10-09T10:30:45Z"
}
```

**Status Codes**:
- `201 Created` - User created successfully
- `409 CONFLICT` - Username already exists
- `422 VALIDATION_ERROR` - Invalid input

### Logout

Invalidates the current session.

```http
POST /api/auth/logout
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Logged out successfully"
}
```

### Verify Session

Verifies if the current token is valid.

```http
GET /api/auth/verify
Authorization: Bearer <token>
```

**Response**:
```json
{
  "valid": true,
  "user": {
    "id": 1,
    "username": "admin"
  },
  "expires_at": "2025-10-10T10:30:45Z"
}
```

### Change Password

Changes the user's password.

```http
POST /api/auth/password
Authorization: Bearer <token>
Content-Type: application/json

{
  "old_password": "current_password",
  "new_password": "new_secure_password"
}
```

**Response**:
```json
{
  "message": "Password changed successfully"
}
```

**Status Codes**:
- `200 OK` - Password changed
- `401 AUTHENTICATION_FAILED` - Old password incorrect
- `422 VALIDATION_ERROR` - New password doesn't meet requirements

### API Keys

#### Create API Key

Generates a new API key for programmatic access.

```http
POST /api/users/{username}/api-keys
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "CI/CD Pipeline",
  "expires_in_days": 90
}
```

**Response**:
```json
{
  "key_id": "key_abc123",
  "key": "hx_1234567890abcdef...",
  "name": "CI/CD Pipeline",
  "created_at": "2025-10-09T10:30:45Z",
  "expires_at": "2026-01-07T10:30:45Z"
}
```

**Note**: The `key` field is only returned once during creation. Store it securely.

#### List API Keys

Lists all API keys for a user (excluding the actual keys).

```http
GET /api/users/{username}/api-keys
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "key_id": "key_abc123",
    "name": "CI/CD Pipeline",
    "created_at": "2025-10-09T10:30:45Z",
    "expires_at": "2026-01-07T10:30:45Z",
    "last_used": "2025-10-09T15:22:10Z"
  }
]
```

#### Revoke API Key

Revokes an API key.

```http
DELETE /api/users/{username}/api-keys/{key_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "API key revoked successfully"
}
```

### Using API Keys

API keys can be used instead of JWT tokens:

```http
GET /api/vms
X-API-Key: hx_1234567890abcdef...
```

---

## Virtual Machines

### List VMs

Retrieves all virtual machines.

```http
GET /api/vms
Authorization: Bearer <token>
```

**Query Parameters**:
- `status` (optional) - Filter by status: `running`, `stopped`, `paused`
- `hypervisor` (optional) - Filter by hypervisor: `qemu`, `lxd`, `incus`
- `limit` (optional) - Maximum number of results (default: 100)
- `offset` (optional) - Pagination offset (default: 0)

**Response**:
```json
[
  {
    "id": "100",
    "name": "web-server-01",
    "hypervisor": "qemu",
    "status": "running",
    "memory": 4096,
    "cpus": 2,
    "disk_size": 53687091200,
    "architecture": "x86_64",
    "disks": [
      {
        "path": "/dev/zvol/tank/vm-100-disk-0",
        "size": 53687091200,
        "format": "raw"
      }
    ],
    "created_at": "2025-10-09T10:30:45Z",
    "uptime_seconds": 3600
  }
]
```

### Get VM

Retrieves details for a specific VM.

```http
GET /api/vms/{id}
Authorization: Bearer <token>
```

**Response**: Same as individual VM object in list response.

**Status Codes**:
- `200 OK` - VM found
- `404 NOT_FOUND` - VM does not exist

### Create VM

Creates a new virtual machine.

```http
POST /api/vms
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "web-server-02",
  "hypervisor": "qemu",
  "memory": 4096,
  "cpus": 2,
  "disk_size": 53687091200,
  "architecture": "x86_64",
  "ostype": "linux",
  "boot_order": "disk,network",
  "network": {
    "bridge": "vmbr0",
    "model": "virtio"
  }
}
```

**Response**:
```json
{
  "id": "101",
  "name": "web-server-02",
  "status": "stopped",
  "message": "VM created successfully"
}
```

**Status Codes**:
- `201 Created` - VM created successfully
- `409 CONFLICT` - VM with this name already exists
- `422 VALIDATION_ERROR` - Invalid configuration, including an `id` that is
  not 1-64 ASCII letters, digits and `-` (not starting with `-`); the ID
  names the VM's disk files and pidfile

### Start VM

Starts a stopped VM.

```http
POST /api/vms/{id}/start
Authorization: Bearer <token>
```

**Response**:
```json
{
  "id": "100",
  "status": "running",
  "message": "VM started successfully",
  "pid": 12345
}
```

**Status Codes**:
- `200 OK` - VM started
- `409 CONFLICT` - VM already running
- `404 NOT_FOUND` - VM does not exist
- `500 INTERNAL_ERROR` - Failed to start VM

### Stop VM

Stops a running VM.

```http
POST /api/vms/{id}/stop
Authorization: Bearer <token>
Content-Type: application/json

{
  "force": false,
  "timeout": 60
}
```

**Parameters**:
- `force` (optional) - Force stop (kill) instead of graceful shutdown (default: false)
- `timeout` (optional) - Seconds to wait before force stop (default: 60)

**Response**:
```json
{
  "id": "100",
  "status": "stopped",
  "message": "VM stopped successfully"
}
```

### Delete VM

Deletes a VM and its associated disks.

```http
DELETE /api/vms/{id}
Authorization: Bearer <token>
```

**Query Parameters**:
- `purge` (optional) - Delete disk images (default: true)

**Response**:
```json
{
  "message": "VM deleted successfully"
}
```

**Status Codes**:
- `200 OK` - VM deleted
- `409 CONFLICT` - VM is running
- `404 NOT_FOUND` - VM does not exist

---

### VM hardware: network cards, CD-ROM, boot order

`POST /api/vms` and `PUT /api/vms/:id/config` take a `hardware` object (QEMU VMs):

```json
{
  "nets":  [{"model": "virtio", "bridge": "vmbr0", "vlan": 20, "mac": "02:..:..:..:..:.."}],
  "cdrom": "/var/lib/horcrux/images/debian-12.iso",
  "boot":  ["cdrom", "disk"]
}
```

- Each NIC is a tap device (`tap<hash>i<n>`) plugged into the host bridge; `vlan`
  sets the tap's PVID and needs a VLAN-aware bridge. A missing `mac` is generated
  (locally administered). `model`: `virtio` (default), `e1000`, `rtl8139`.
- `cdrom` must be a file in the ISO library (see ISO Images). It can be changed or
  set to `null` (eject) while the VM runs; everything else needs the VM stopped
  (409 otherwise).
- `boot`: any order of `disk`, `cdrom`, `net`; default disk then CD-ROM.
- `PUT /api/vms/:id/config` also takes `name`, `cpus`, `memory` (MB); absent
  fields are kept.
- `GET /api/vms/host-options`: the host's bridges (`name`, `vlan_aware`), the ISO
  library and its directory, for VM forms.

### Shutdown / Reboot / Reset VM

- `POST /api/vms/:id/shutdown` `{"timeout": 60, "force_stop": false}`: ACPI power
  button, wait up to `timeout` s. If the guest ignores it: 409 (VM keeps running),
  or with `force_stop` the VM is stopped.
- `POST /api/vms/:id/reboot` `{"timeout": 60}`: shutdown (stopped after the
  timeout), then start.
- `POST /api/vms/:id/reset`: hard reset.

### ISO Images

- `GET /api/storage/isos`: ISOs in the image directory (`HORCRUX_IMAGE_DIR`,
  default `<data_dir>/images`) and storage pool directories; downloads in
  progress show `downloading: true`, failed ones an `error`.
- `POST /api/storage/isos/download` `{"url": "https://...", "filename": "x.iso"}`
  -> 202; the file appears once complete.
- `DELETE /api/storage/isos/:name` (409 while a VM's CD-ROM holds it).

### Host bridges

- `GET /api/network/bridges`: every Linux bridge (ports, addresses, VLAN
  filtering, `managed`).
- `POST /api/network/bridges` `{"name": "vmbr0", "vlan_aware": true,
  "address": "10.10.0.1/24", "ports": ["eth1"]}`: create a bridge; Horcrux
  re-creates it when it starts. A port that carries an IP address is refused.
- `DELETE /api/network/bridges/:name`: managed bridges only, refused while a VM
  NIC uses it.

## Snapshots

### List Snapshots

Lists all snapshots for a VM.

```http
GET /api/vms/{id}/snapshots
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "snap-20251009-103045",
    "vm_id": "100",
    "name": "before-upgrade",
    "description": "Pre-upgrade snapshot",
    "created_at": "2025-10-09T10:30:45Z",
    "include_memory": true,
    "disk_snapshots": [
      {
        "disk_path": "/dev/zvol/tank/vm-100-disk-0",
        "snapshot_name": "tank/vm-100-disk-0@snap-20251009-103045",
        "size": 1073741824
      }
    ],
    "parent_snapshot": null
  }
]
```

### Create Snapshot

Creates a snapshot of a VM.

```http
POST /api/vms/{id}/snapshots
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "before-upgrade",
  "description": "Pre-upgrade snapshot",
  "include_memory": true
}
```

**Parameters**:
- `name` (required) - Snapshot name (alphanumeric, hyphens, underscores)
- `description` (optional) - Description of the snapshot
- `include_memory` (optional) - Include RAM state (default: false, only for running VMs)

**Response**:
```json
{
  "id": "snap-20251009-103045",
  "vm_id": "100",
  "name": "before-upgrade",
  "created_at": "2025-10-09T10:30:45Z",
  "message": "Snapshot created successfully"
}
```

**Status Codes**:
- `201 Created` - Snapshot created
- `404 NOT_FOUND` - VM does not exist
- `422 VALIDATION_ERROR` - Invalid snapshot name
- `500 INTERNAL_ERROR` - Snapshot creation failed

### Get Snapshot

Gets details for a specific snapshot.

```http
GET /api/vms/{id}/snapshots/{snapshot_id}
Authorization: Bearer <token>
```

**Response**: Same as individual snapshot object in list response.

### Restore Snapshot

Restores a VM to a previous snapshot state.

```http
POST /api/vms/{id}/snapshots/{snapshot_id}/restore
Authorization: Bearer <token>
Content-Type: application/json

{
  "restore_memory": true
}
```

**Parameters**:
- `restore_memory` (optional) - Restore RAM state if available (default: true)

**Response**:
```json
{
  "message": "VM restored to snapshot successfully",
  "snapshot_id": "snap-20251009-103045",
  "vm_status": "running"
}
```

**Status Codes**:
- `200 OK` - Snapshot restored
- `404 NOT_FOUND` - VM or snapshot does not exist
- `409 CONFLICT` - VM must be stopped for restore

### Delete Snapshot

Deletes a snapshot.

```http
DELETE /api/vms/{id}/snapshots/{snapshot_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Snapshot deleted successfully"
}
```

### Get Snapshot Tree

Retrieves the snapshot hierarchy for a VM.

```http
GET /api/vms/{id}/snapshots/tree
Authorization: Bearer <token>
```

**Response**:
```json
{
  "vm_id": "100",
  "snapshots": [
    {
      "id": "snap-1",
      "name": "initial",
      "created_at": "2025-10-01T10:00:00Z",
      "children": [
        {
          "id": "snap-2",
          "name": "after-install",
          "created_at": "2025-10-02T10:00:00Z",
          "children": []
        }
      ]
    }
  ]
}
```

---

## Cloning

### Clone VM

Creates a clone of an existing VM.

```http
POST /api/vms/{id}/clone
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "web-server-clone",
  "id": "102",
  "mode": "full",
  "description": "Production clone",
  "start": false,
  "snapshot": "snap-20251009-103045",
  "mac_addresses": ["52:54:00:12:34:57"]
}
```

**Parameters**:
- `name` (required) - Name for the cloned VM
- `id` (optional) - VM ID for the clone (auto-generated if not provided); same rules as a Create VM `id` (422 otherwise)
- `mode` (optional) - Clone mode: `full` (independent copy) or `linked` (COW snapshot, QCOW2 only) (default: `full`)
- `description` (optional) - Description for the clone
- `start` (optional) - Start the VM after cloning (default: false)
- `snapshot` (optional) - Clone from specific snapshot instead of current state
- `mac_addresses` (optional) - Custom MAC addresses for network interfaces

**Response**:
```json
{
  "id": "102",
  "name": "web-server-clone",
  "status": "stopped",
  "message": "VM cloned successfully",
  "mode": "full",
  "source_vm": "100"
}
```

**Clone Modes**:
- **Full Clone**: Creates a completely independent copy with its own disk images. Works with all storage backends (ZFS, LVM, Btrfs, Ceph, QCOW2).
- **Linked Clone**: Uses copy-on-write to share the original disk as a backing file. Only works with QCOW2. Much faster and uses less space initially.

**Status Codes**:
- `201 Created` - Clone created successfully
- `404 NOT_FOUND` - Source VM does not exist
- `422 VALIDATION_ERROR` - Invalid mode or configuration
- `500 INTERNAL_ERROR` - Clone operation failed

---

## Cloud-Init

Cloud-Init enables automated VM provisioning and configuration.

### Generate Cloud-Init ISO

Generates a cloud-init configuration ISO for a VM.

```http
POST /api/cloudinit/{vm_id}
Authorization: Bearer <token>
Content-Type: application/json

{
  "user_data": {
    "users": [
      {
        "name": "admin",
        "password": "hashed_password",
        "ssh_authorized_keys": [
          "ssh-rsa AAAAB3NzaC1yc2E..."
        ],
        "sudo": "ALL=(ALL) NOPASSWD:ALL"
      }
    ],
    "packages": ["nginx", "postgresql", "redis"],
    "runcmd": [
      "systemctl enable nginx",
      "systemctl start nginx"
    ]
  },
  "network_config": {
    "version": 2,
    "ethernets": {
      "eth0": {
        "addresses": ["192.168.1.100/24"],
        "gateway4": "192.168.1.1",
        "nameservers": {
          "addresses": ["8.8.8.8", "8.8.4.4"]
        }
      }
    }
  },
  "meta_data": {
    "instance_id": "vm-100",
    "local_hostname": "web-server-01"
  }
}
```

**Response**:
```json
{
  "vm_id": "100",
  "iso_path": "/var/lib/horcrux/cloudinit/vm-100-cloudinit.iso",
  "message": "Cloud-init ISO generated successfully"
}
```

**User Data Format**:
The `user_data` field supports standard cloud-config YAML directives:
- `users` - User accounts to create
- `packages` - Packages to install
- `runcmd` - Commands to run on first boot
- `write_files` - Files to create
- `bootcmd` - Commands to run early in boot
- `ssh_authorized_keys` - Global SSH keys

**Network Config Format**:
Uses Netplan v2 format for network configuration.

**Status Codes**:
- `201 Created` - ISO generated successfully
- `404 NOT_FOUND` - VM does not exist
- `422 VALIDATION_ERROR` - Invalid cloud-init configuration
- `500 INTERNAL_ERROR` - ISO generation failed

### Delete Cloud-Init ISO

Removes the cloud-init ISO for a VM.

```http
DELETE /api/cloudinit/{vm_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Cloud-init ISO deleted successfully"
}
```

---

## Backups

Backups are real vzdump-style archives: a tar (zstd/gzip/lzo/none) holding
`horcrux-backup.json` (guest config + SHA-256 of every file) and the payload:
`disk-N.qcow2` for VMs (every disk, flattened with `qemu-img convert`),
`rootfs.tar` (+ `inspect.json`) for Docker/Podman/LXC containers, or the
`lxc`/`incus export` tarball for LXD/Incus. The catalog and jobs are stored in
the database. A stopped guest is copied offline in every mode. For a running
guest, `stop` stops it, backs it up and restarts it; `suspend` and `snapshot`
pause it for the copy (VMs via QMP `stop`, which flushes every disk;
containers are frozen) and resume it afterwards. There is no no-pause
copy-before-write mode yet.

`storage` is `local` (the default backup directory, `paths.backups`), the id
of a directory/NFS/CIFS/BtrFS/GlusterFS/ZFS storage pool (archives go to
`<pool path>/dump`), or an absolute directory.

| Method | Path | Body / query | Result |
|---|---|---|---|
| GET | `/api/backups` | `?target_id=` | `[Backup]`, newest first |
| POST | `/api/backups` | `{"target_id", "target_type"?: "vm"/"container", "storage"?, "mode"?: "snapshot"/"suspend"/"stop", "compression"?: "zstd"/"gzip"/"lzo"/"none", "notes"?, "protected"?}` | 201 `Backup` (runs to completion) |
| GET | `/api/backups/:id` | | `Backup` |
| PUT | `/api/backups/:id` | `{"notes"?, "protected"?}` | `Backup` |
| DELETE | `/api/backups/:id` | | 204; 409 if protected |
| POST | `/api/backups/:id/verify` | | `{"backup_id","ok","checked_at","files_checked","error"}`, also stored on the backup as `verify_state`/`verified_at`/`verify_error` |
| POST | `/api/backups/:id/restore` | `{"target_id"?, "name"?, "force"?, "start"?}` | `{"backup_id","target_type","target_id","name","replaced"}`; 409 if the target is running, or is a different existing guest and `force` is not set |
| POST | `/api/backups/prune` | `{"target_id", "storage"?, "keep_last"?, "keep_hourly"?, "keep_daily"?, "keep_weekly"?, "keep_monthly"?, "keep_yearly"?, "dry_run"?}` | `[{"backup_id","timestamp","mark": "keep"/"remove"/"protected"}]` |
| POST | `/api/backups/retention/:target_id` | keep-* policy | same as prune (not a dry run) |
| GET | `/api/backup-storages` | | `[{"id","path","kind","available_gb","total_gb"}]` |

`Backup`: `id, target_type, target_id, target_name, timestamp, size, mode,
compression, path, storage, config_included, notes, protected, sha256,
job_id, verify_state, verified_at, verify_error`.

Restore verifies the archive (SHA-256, manifest checksums, `qemu-img check`)
before it touches anything. A VM restore re-registers the VM: the same id
replaces it, a new id creates a new VM next to the original.

Retention follows Proxmox `prune-backups`: the options apply in the order
last, hourly, daily, weekly, monthly, yearly; each keeps the newest backup of
N periods not already covered. Protected backups are always kept.

### Backup Jobs

| Method | Path | Body | Result |
|---|---|---|---|
| GET | `/api/backup-jobs` | | `[BackupJob]` |
| POST | `/api/backup-jobs` | `BackupJob` | 201 `BackupJob` |
| GET / PUT / DELETE | `/api/backup-jobs/:id` | `BackupJob` (PUT) | `BackupJob` / 204 |
| POST | `/api/backup-jobs/:id/run` | | 202 `{"job_id","started"}` (runs in the background) |

`BackupJob`: `{"id"?, "comment"?, "enabled"?: true, "schedule", "targets":
[ids], "storage"?, "mode"?, "compression"?, "retention"?: {keep-*}, "notify"?:
"always"/"failure"/"never"}`, plus the server-set `last_run`, `last_status`
("ok"/"failed"), `last_error` and `next_run`. `schedule` is a 5-field cron
expression in local time (`*/15 9-17 * * mon-fri`) or `hourly`, `daily`,
`weekly`, `monthly`, `yearly`. The server's scheduler runs due jobs. After
each run the job's retention is applied per guest and storage, and the result
goes to the configured notification channels as `notify` says.

## Templates

### List Templates

Lists all VM templates.

```http
GET /api/templates
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "tmpl-ubuntu-22.04",
    "name": "Ubuntu 22.04 LTS",
    "description": "Ubuntu 22.04 LTS base template",
    "ostype": "linux",
    "architecture": "x86_64",
    "memory": 2048,
    "cpus": 2,
    "disk_size": 21474836480,
    "created_at": "2025-10-01T10:00:00Z"
  }
]
```

### Create Template

Creates a VM template from an existing VM.

```http
POST /api/templates
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "100",
  "name": "My Custom Template",
  "description": "Custom configured web server"
}
```

**Response**:
```json
{
  "id": "tmpl-custom-1",
  "name": "My Custom Template",
  "message": "Template created successfully"
}
```

### Get Template

Gets details for a specific template.

```http
GET /api/templates/{id}
Authorization: Bearer <token>
```

**Response**: Same as individual template object in list response.

### Clone Template

Creates a VM from a template.

```http
POST /api/templates/{id}/clone
Authorization: Bearer <token>
Content-Type: application/json

{
  "new_vm_id": "web-03",
  "new_vm_name": "web-server-03",
  "clone_type": "linked"
}
```

`clone_type` is `full` (standalone copy) or `linked` (qcow2 overlay on the
template disk). `new_vm_id` follows the Create VM `id` rule: 1-64 ASCII
letters, digits and `-`, not starting with `-` (`422 VALIDATION_ERROR`
otherwise; it names the clone's disk file). `409` if a VM with that ID exists.

**Response**:
```json
{
  "new_vm_id": "web-03"
}
```

### Delete Template

Deletes a template.

```http
DELETE /api/templates/{id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Template deleted successfully"
}
```

---

## Storage

### List Storage Pools

Lists all storage pools.

```http
GET /api/storage/pools
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "local",
    "name": "local",
    "type": "zfs",
    "path": "/tank/vms",
    "total": 1099511627776,
    "available": 549755813888,
    "used": 549755813888,
    "status": "online"
  }
]
```

### Get Storage Pool

Gets details for a specific storage pool.

```http
GET /api/storage/pools/{id}
Authorization: Bearer <token>
```

**Response**: Same as individual pool object in list response.

### Add Storage Pool

Adds a new storage pool.

```http
POST /api/storage/pools
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "fast-ssd",
  "type": "lvm",
  "path": "/dev/vg-fast/lv-storage",
  "options": {
    "thin": true
  }
}
```

**Supported Types**:
- `zfs` - ZFS filesystem
- `lvm` - LVM logical volumes
- `dir` - Directory-based storage
- `nfs` - NFS network storage
- `cifs` - CIFS/SMB network storage
- `ceph` - Ceph RBD
- `glusterfs` - GlusterFS

**Response**:
```json
{
  "id": "fast-ssd",
  "name": "fast-ssd",
  "message": "Storage pool added successfully"
}
```

### Remove Storage Pool

Removes a storage pool.

```http
DELETE /api/storage/pools/{id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Storage pool removed successfully"
}
```

### Create Volume

Creates a storage volume.

```http
POST /api/storage/pools/{pool_id}/volumes
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "vm-disk-1",
  "size": 53687091200,
  "format": "raw"
}
```

**Response**:
```json
{
  "path": "/dev/zvol/tank/vm-disk-1",
  "size": 53687091200,
  "message": "Volume created successfully"
}
```

---

## Networking

### CNI Networks

#### List CNI Networks

Lists all CNI networks.

```http
GET /api/cni/networks
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "name": "default-bridge",
    "type": "bridge",
    "bridge": "cni0",
    "subnet": "10.22.0.0/16",
    "gateway": "10.22.0.1",
    "dns": {
      "nameservers": ["8.8.8.8"]
    }
  }
]
```

#### Create CNI Network

Creates a new CNI network.

```http
POST /api/cni/networks
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "web-network",
  "type": "bridge",
  "bridge": "web0",
  "subnet": "10.50.0.0/24",
  "gateway": "10.50.0.1",
  "ipam": {
    "type": "host-local",
    "ranges": [
      [{"subnet": "10.50.0.0/24"}]
    ]
  }
}
```

**Response**:
```json
{
  "name": "web-network",
  "message": "CNI network created successfully"
}
```

#### Get CNI Network

Gets details for a specific CNI network.

```http
GET /api/cni/networks/{name}
Authorization: Bearer <token>
```

#### Delete CNI Network

Deletes a CNI network.

```http
DELETE /api/cni/networks/{name}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "CNI network deleted successfully"
}
```

#### Attach Container to Network

Runs the network's CNI plugin (ADD) for a container and records the
attachment. Give either a Horcrux container id (its network namespace is
looked up from Docker/Podman/LXC/LXD/Incus; the container must be running)
or a raw `netns_path` (`/proc/<pid>/ns/net` or `/run/netns/<name>`).
`interface_name` defaults to the first free `netN`; `namespace` (default
`default`) and `labels` are what network policies select on.

```http
POST /api/cni/attach
Authorization: Bearer ***
Content-Type: application/json

{
  "container_id": "web1",
  "network_name": "web-network",
  "namespace": "default",
  "labels": {"app": "web"}
}
```

**Response** (409 if already attached, 404 for an unknown network or container):
```json
{
  "container_id": "web1",
  "network_name": "web-network",
  "interface_name": "net0",
  "ip_address": "10.50.0.2",
  "mac_address": "6a:0b:3c:11:22:33",
  "gateway": "10.50.0.1",
  "netns_path": "/proc/4242/ns/net",
  "guest": "web1",
  "namespace": "default",
  "labels": {"app": "web"}
}
```

`GET /api/cni/attachments` lists every attachment, `GET
/api/cni/attachments/{container_id}` one container's. Deleting a network
that still has attachments answers 409.

#### Detach Container from Network

Runs the plugin's DEL with the recorded interface and namespace (this also
releases the host-local IP lease) and forgets the attachment.

```http
POST /api/cni/detach
Authorization: Bearer ***
Content-Type: application/json

{
  "container_id": "web1",
  "network_name": "web-network"
}
```

### Network Policies

Kubernetes-style policies over CNI attachments. A policy selects the
attachments in its `namespace` whose labels match `pod_selector`; for each
direction in `policy_types` (default: Ingress, plus Egress when there are
egress rules) only traffic matching one of its rules is allowed. They are
enforced in the kernel in the nftables table `bridge horcrux_netpol`,
rebuilt after every policy or attachment change and at startup; policies,
networks and attachments are stored in the database.

#### List Network Policies

```http
GET /api/network-policies
Authorization: Bearer ***
```

#### Create Network Policy

`id` is generated when empty. Peers are `{"PodSelector": {...}}`,
`{"NamespaceSelector": {...}}` or `{"IpBlock": {"cidr": "...", "except": []}}`.

```http
POST /api/network-policies
Authorization: Bearer ***
Content-Type: application/json

{
  "id": "db-from-web",
  "name": "db only from web",
  "namespace": "default",
  "pod_selector": {"match_labels": {"app": "db"}, "match_expressions": []},
  "policy_types": ["Ingress"],
  "ingress": [
    {
      "from": [{"PodSelector": {"match_labels": {"app": "web"}, "match_expressions": []}}],
      "ports": [{"protocol": "TCP", "port": 5432, "end_port": null}]
    }
  ],
  "egress": [],
  "enabled": true
}
```

**Response** (create, `PUT /api/network-policies/{id}` and delete answer the same way):
```json
{
  "message": "Network policy created",
  "enforcement": {
    "applied": true,
    "table": "bridge horcrux_netpol",
    "stateful": true,
    "endpoints": 1,
    "rules": 1,
    "error": null,
    "applied_at": "2026-10-21T13:05:47Z"
  }
}
```

`GET /api/network-policies/enforcement` returns the last `enforcement`
result; `POST` to it installs the rules again.

#### Get / Update / Delete Network Policy

```http
GET /api/network-policies/{id}
PUT /api/network-policies/{id}
DELETE /api/network-policies/{id}
Authorization: Bearer ***
```

#### Get Policy Rules

`GET /api/network-policies/{id}/nftables` lists the enforced chains this
policy contributes to (one per selected attachment and direction: its allow
rules and the final drop). `GET /api/network-policies/{id}/iptables` returns
an equivalent iptables translation as text (not applied).

---

## Console Access

### Create VNC Console

Creates a VNC console session for a VM.

```http
POST /api/console/{vm_id}/vnc
Authorization: Bearer <token>
```

**Response**:
```json
{
  "vm_id": "100",
  "protocol": "vnc",
  "host": "127.0.0.1",
  "port": 5900,
  "ticket": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "ws_port": 6080,
  "expires_at": "2025-10-09T11:30:45Z"
}
```

### Get VNC WebSocket URL

Gets the WebSocket URL for browser-based console access.

```http
GET /api/console/{vm_id}/websocket
Authorization: Bearer <token>
```

**Response**:
```json
{
  "websocket_url": "ws://127.0.0.1:6080/a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

**Usage**: Use with noVNC or spice-html5 client in browser.

### Verify Console Ticket

Verifies a console access ticket.

```http
GET /api/console/ticket/{ticket_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "valid": true,
  "vm_id": "100",
  "protocol": "vnc",
  "expires_at": "2025-10-09T11:30:45Z"
}
```

---

## Monitoring

### Get Node Stats

Gets system resource statistics for the node.

```http
GET /api/monitoring/node
Authorization: Bearer <token>
```

**Response**:
```json
{
  "hostname": "horcrux-node-1",
  "cpu_usage": 45.2,
  "cpu_cores": 8,
  "memory_total": 17179869184,
  "memory_used": 10737418240,
  "memory_usage": 62.5,
  "disk_total": 1099511627776,
  "disk_used": 549755813888,
  "disk_usage": 50.0,
  "network_rx_bytes": 1073741824,
  "network_tx_bytes": 536870912,
  "uptime_seconds": 86400,
  "load_average": [1.5, 1.8, 2.1]
}
```

### Get VM Stats

Gets resource statistics for a specific VM.

```http
GET /api/monitoring/vms/{id}
Authorization: Bearer <token>
```

Refreshed every 10 seconds for running VMs (`null` if the VM isn't running
or has no metrics source yet). Rates are per second over the last interval;
memory and disk totals are the VM's configured sizes, disk `used_bytes` is
what its image files actually occupy on the host.

**Response**:
```json
{
  "id": "vm-100",
  "name": "web-server",
  "timestamp": 1760796000,
  "cpu": { "usage_percent": 25.3, "cores": 2, "load_average": 0.51 },
  "memory": {
    "total_bytes": 4294967296,
    "used_bytes": 2147483648,
    "free_bytes": 2147483648,
    "usage_percent": 50.0
  },
  "disk": {
    "read_bytes_per_sec": 1048576,
    "write_bytes_per_sec": 524288,
    "read_iops": 0,
    "write_iops": 0,
    "total_bytes": 21474836480,
    "used_bytes": 3221225472
  },
  "network": {
    "rx_bytes_per_sec": 0,
    "tx_bytes_per_sec": 0,
    "rx_packets_per_sec": 0,
    "tx_packets_per_sec": 0
  }
}
```

### Get All VM Stats

Gets statistics for all VMs.

```http
GET /api/monitoring/vms
Authorization: Bearer <token>
```

**Response**: Array of VM stats objects.

### Get Metric History

Gets historical metrics for a specific metric.

```http
GET /api/monitoring/history/{metric}
Authorization: Bearer <token>
```

**Query Parameters**:
- `start` - Start timestamp (ISO 8601)
- `end` - End timestamp (ISO 8601)
- `interval` - Sample interval in seconds (default: 60)

**Metrics**:
- `cpu_usage`
- `memory_usage`
- `disk_usage`
- `network_rx`
- `network_tx`

**Response**:
```json
{
  "metric": "cpu_usage",
  "start": "2025-10-09T10:00:00Z",
  "end": "2025-10-09T11:00:00Z",
  "interval": 60,
  "data": [
    {"timestamp": "2025-10-09T10:00:00Z", "value": 45.2},
    {"timestamp": "2025-10-09T10:01:00Z", "value": 47.8},
    {"timestamp": "2025-10-09T10:02:00Z", "value": 43.1}
  ]
}
```

### Prometheus Metrics

Exposes metrics in Prometheus format.

```http
GET /metrics
```

**Response**: Prometheus text format.

---

## Clustering

### List Cluster Nodes

Lists all nodes in the cluster. `cpu_usage` (percent) and `memory_used`
(bytes) are live figures from the node heartbeat: every 10 s each
horcrux-api polls every registered node's `GET /api/cluster/local-usage`
(this host is sampled directly from procfs) and updates the node's usage
and `status` (`online` when it answers; `offline` after 30 s without an
answer). They are `null` when unknown -- the node is unreachable, has never
reported, or rejected the poll (nodes must share `JWT_SECRET`) -- and are
never reported as 0 in that case. `last_seen` is the Unix time of the last
successful report. `cpu_cores`, `memory_total` and an `unknown`
architecture are filled in from the node's own report.

```http
GET /api/cluster/nodes
Authorization: Bearer ***
```

**Response**:
```json
[
  {
    "id": 1,
    "name": "horcrux-node-1",
    "ip": "192.168.1.10",
    "status": "online",
    "priority": 100,
    "is_local": false,
    "architecture": "x86_64",
    "cpu_cores": 8,
    "memory_total": 17179869184,
    "cpu_usage": 12.5,
    "memory_used": 4294967296,
    "last_seen": 1760572800
  },
  {
    "id": 2,
    "name": "horcrux-node-2",
    "ip": "192.168.1.11",
    "status": "offline",
    "priority": 100,
    "is_local": false,
    "architecture": "aarch64",
    "cpu_cores": 4,
    "memory_total": 8589934592,
    "cpu_usage": null,
    "memory_used": null,
    "last_seen": null
  }
]
```

### Get Cluster Status

Cluster-wide summary (`horcrux cluster status`). `name` is the Corosync
cluster name (`null` if none is configured); `has_quorum` is `null` when
quorum can't be determined (`corosync-quorumtool` unavailable).
`total_vms` counts VMs managed by the answering server. `memory_used` sums
the live usage of the `nodes_reporting_usage` nodes that report it and is
`null` if none do.

```http
GET /api/cluster/status
Authorization: Bearer ***
```

**Response**:
```json
{
  "name": null,
  "total_nodes": 2,
  "online_nodes": 1,
  "has_quorum": null,
  "total_vms": 3,
  "total_cpus": 12,
  "total_memory": 25769803776,
  "memory_used": 4294967296,
  "nodes_reporting_usage": 1
}
```

### Get This Node's Live Usage

This server's own live usage, sampled from procfs (CPU is a real delta
between consecutive `/proc/stat` reads; memory is MemTotal - MemAvailable).
Polled by the other cluster nodes' heartbeat.

```http
GET /api/cluster/local-usage
Authorization: Bearer ***
```

**Response**:
```json
{
  "cpu_usage": 12.5,
  "memory_used": 4294967296,
  "memory_total": 17179869184,
  "cpu_cores": 8,
  "architecture": "x86_64",
  "timestamp": 1760572800
}
```

`503` if procfs can't be read.

### Add Cluster Node

Adds a new node to the cluster. The node name comes from the path. Only
`address` (alias `ip`) is required; `architecture` defaults to `unknown`,
`priority` (placement/HA weight, 0-1000) to 100, and `cpu_cores` /
`memory_total` (bytes) to 0 until known.

```http
POST /api/cluster/nodes/{name}
Authorization: Bearer ***
Content-Type: application/json

{
  "address": "192.168.1.11",
  "architecture": "x86_64",
  "priority": 100
}
```

**Response**: `201 Created` (no body).

### Set Node Weight

```http
PATCH /api/cluster/nodes/{name}/priority
Content-Type: application/json

{ "priority": 300 }
```

**Response**: `200 OK` (no body).

### Remove Cluster Node

```http
DELETE /api/cluster/nodes/{name}
Authorization: Bearer ***
```

**Response**: `204 No Content`; `404` if no such node; `409` for the local
node.

### Get Cluster Architecture

Node counts per CPU architecture.

```http
GET /api/cluster/architecture
Authorization: Bearer ***
```

**Response**:
```json
{
  "x86_64_nodes": 2,
  "aarch64_nodes": 1,
  "other_nodes": 0,
  "is_mixed": true
}
```

### Find Best Node for VM

Picks the best online node that can run the architecture and has the
requested resources, preferring native architecture, then higher weight.

```http
POST /api/cluster/find-node
Authorization: Bearer ***
Content-Type: application/json

{
  "architecture": "x86_64",
  "memory_mb": 4096,
  "cpu_cores": 2,
  "min_priority": 100
}
```

`min_priority` is optional.

**Response**: the node name as a JSON string, e.g. `"horcrux-node-2"`.

---

## High Availability

### List HA Resources

Lists all high availability resources.

```http
GET /api/ha/resources
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "vm_id": "100",
    "priority": 100,
    "state": "started",
    "current_node": "horcrux-node-1",
    "preferred_node": "horcrux-node-1",
    "group": "critical-services"
  }
]
```

### Add HA Resource

Adds a VM to high availability management.

```http
POST /api/ha/resources
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "100",
  "priority": 100,
  "group": "critical-services",
  "preferred_node": "horcrux-node-1"
}
```

**Priority**: Higher values = higher priority (0-255)

**Response**:
```json
{
  "vm_id": "100",
  "message": "VM added to HA management"
}
```

### Remove HA Resource

Removes a VM from HA management.

```http
DELETE /api/ha/resources/{vm_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "VM removed from HA management"
}
```

### Get HA Status

Gets the overall HA status.

```http
GET /api/ha/status
Authorization: Bearer <token>
```

**Response**:
```json
{
  "enabled": true,
  "total_resources": 5,
  "resources_started": 5,
  "resources_stopped": 0,
  "failover_count_24h": 1
}
```

### Create HA Group

Creates a high availability group.

```http
POST /api/ha/groups
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "critical-services",
  "nodes": ["horcrux-node-1", "horcrux-node-2"],
  "nofailback": false
}
```

**Response**:
```json
{
  "name": "critical-services",
  "message": "HA group created successfully"
}
```

### List HA Groups

Lists all HA groups.

```http
GET /api/ha/groups
Authorization: Bearer <token>
```

---

## Migration

### Migrate VM

Initiates VM migration to another node.

```http
POST /api/migrate/{vm_id}
Authorization: Bearer <token>
Content-Type: application/json

{
  "target_node": "horcrux-node-2",
  "mode": "live",
  "bandwidth_limit_mbps": 100,
  "timeout_seconds": 300
}
```

**Migration Modes**:
- `live` - Live migration with minimal downtime (VM stays running)
- `offline` - Offline migration (VM must be stopped)
- `online` - Online migration (stops VM, transfers, starts on target)

**Parameters**:
- `target_node` (required) - Destination node name
- `mode` (optional) - Migration mode (default: `online`)
- `bandwidth_limit_mbps` (optional) - Network bandwidth limit (default: unlimited)
- `timeout_seconds` (optional) - Migration timeout (default: 300)

**Response**:
```json
{
  "job_id": "migration-12345",
  "vm_id": "100",
  "status": "preparing",
  "target_node": "horcrux-node-2",
  "mode": "live",
  "started_at": "2025-10-09T10:30:45Z"
}
```

### Get Migration Status

Gets the status of an ongoing migration.

```http
GET /api/migrate/{vm_id}/status
Authorization: Bearer <token>
```

**Response**:
```json
{
  "job_id": "migration-12345",
  "vm_id": "100",
  "status": "transferring",
  "progress": 65,
  "source_node": "horcrux-node-1",
  "target_node": "horcrux-node-2",
  "started_at": "2025-10-09T10:30:45Z",
  "estimated_completion": "2025-10-09T10:35:20Z",
  "transferred_bytes": 3221225472,
  "total_bytes": 4294967296
}
```

**Status Values**:
- `preparing` - Initializing migration
- `transferring` - Transferring disk/memory data
- `syncing` - Final synchronization
- `finalizing` - Completing migration
- `completed` - Migration successful
- `failed` - Migration failed

---

## Firewall

### List Firewall Rules

Lists all firewall rules.

```http
GET /api/firewall/rules
Authorization: Bearer <token>
```

**Query Parameters**:
- `scope` (optional) - Filter by scope: `global`, `node`, `vm`
- `vm_id` (optional) - Filter by VM ID

**Response**:
```json
[
  {
    "id": "rule-1",
    "scope": "vm",
    "vm_id": "100",
    "action": "ACCEPT",
    "direction": "IN",
    "protocol": "tcp",
    "source": "0.0.0.0/0",
    "destination_port": 22,
    "enabled": true,
    "comment": "Allow SSH"
  }
]
```

### Add Firewall Rule

Creates a new firewall rule.

```http
POST /api/firewall/rules
Authorization: Bearer <token>
Content-Type: application/json

{
  "scope": "vm",
  "vm_id": "100",
  "action": "ACCEPT",
  "direction": "IN",
  "protocol": "tcp",
  "source": "0.0.0.0/0",
  "destination_port": 80,
  "comment": "Allow HTTP"
}
```

**Actions**: `ACCEPT`, `DROP`, `REJECT`
**Directions**: `IN`, `OUT`
**Protocols**: `tcp`, `udp`, `icmp`, `all`

**Response**:
```json
{
  "id": "rule-2",
  "message": "Firewall rule added successfully"
}
```

### Delete Firewall Rule

Deletes a firewall rule.

```http
DELETE /api/firewall/rules/{id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Firewall rule deleted successfully"
}
```

### List Security Groups

Lists all security groups.

```http
GET /api/firewall/security-groups
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "name": "web-servers",
    "description": "Web server security group",
    "rules": [
      {
        "action": "ACCEPT",
        "direction": "IN",
        "protocol": "tcp",
        "destination_port": 80
      },
      {
        "action": "ACCEPT",
        "direction": "IN",
        "protocol": "tcp",
        "destination_port": 443
      }
    ]
  }
]
```

### Get Security Group

Gets details for a specific security group.

```http
GET /api/firewall/security-groups/{name}
Authorization: Bearer <token>
```

### Apply Firewall Rules

Applies firewall rules for a specific scope.

```http
POST /api/firewall/{scope}/apply
Authorization: Bearer <token>
```

**Scopes**: `global`, `node`, `vm`

**Response**:
```json
{
  "message": "Firewall rules applied successfully",
  "rules_applied": 15
}
```

---

## GPU Passthrough

### List GPU Devices

Lists all available GPU devices on the node.

```http
GET /api/gpu/devices
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "pci_address": "0000:01:00.0",
    "vendor": "NVIDIA Corporation",
    "device": "GeForce RTX 3080",
    "driver": "nvidia",
    "iommu_group": 13,
    "vfio_bound": false,
    "in_use": false
  }
]
```

### Scan GPU Devices

Rescans for GPU devices.

```http
POST /api/gpu/devices/scan
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "GPU scan completed",
  "devices_found": 2
}
```

### Get GPU Device

Gets details for a specific GPU.

```http
GET /api/gpu/devices/{pci_address}
Authorization: Bearer <token>
```

**Response**: Same as individual GPU object in list response.

### Bind GPU to VFIO

Binds a GPU to the VFIO driver for passthrough.

```http
POST /api/gpu/devices/{pci_address}/bind-vfio
Authorization: Bearer <token>
```

**Response**:
```json
{
  "pci_address": "0000:01:00.0",
  "message": "GPU bound to vfio-pci driver",
  "vfio_bound": true
}
```

### Unbind GPU from VFIO

Unbinds a GPU from VFIO (returns to host).

```http
POST /api/gpu/devices/{pci_address}/unbind-vfio
Authorization: Bearer <token>
```

**Response**:
```json
{
  "pci_address": "0000:01:00.0",
  "message": "GPU unbound from vfio-pci driver",
  "vfio_bound": false
}
```

### Get IOMMU Group

Gets the IOMMU group for a GPU.

```http
GET /api/gpu/devices/{pci_address}/iommu-group
Authorization: Bearer <token>
```

**Response**:
```json
{
  "iommu_group": 13,
  "devices": [
    "0000:01:00.0",
    "0000:01:00.1"
  ]
}
```

### Check IOMMU Status

Checks if IOMMU is enabled on the system.

```http
GET /api/gpu/iommu-status
Authorization: Bearer <token>
```

**Response**:
```json
{
  "enabled": true,
  "type": "intel_iommu"
}
```

---

## Security

### Users

#### List Users

Lists all users.

```http
GET /api/users
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": 1,
    "username": "admin",
    "email": "admin@example.com",
    "role": "Administrator",
    "created_at": "2025-10-01T10:00:00Z",
    "last_login": "2025-10-09T10:30:45Z"
  }
]
```

#### Create User

Creates a new user.

```http
POST /api/users
Authorization: Bearer <token>
Content-Type: application/json

{
  "username": "developer",
  "password": "secure_password",
  "email": "dev@example.com",
  "role": "VmUser"
}
```

**Response**:
```json
{
  "id": 2,
  "username": "developer",
  "message": "User created successfully"
}
```

#### Delete User

Deletes a user.

```http
DELETE /api/users/{id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "User deleted successfully"
}
```

### Roles

#### List Roles

Lists all available roles.

```http
GET /api/roles
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "name": "Administrator",
    "description": "Full system access",
    "privileges": ["*"]
  },
  {
    "name": "VmAdmin",
    "description": "VM management",
    "privileges": ["VmAllocate", "VmConfig", "VmPowerMgmt", "VmSnapshot", "VmBackup", "VmAudit"]
  },
  {
    "name": "VmUser",
    "description": "Basic VM operations",
    "privileges": ["VmPowerMgmt", "VmAudit"]
  },
  {
    "name": "StorageAdmin",
    "description": "Storage management",
    "privileges": ["DatastoreAllocate", "DatastoreAudit", "PoolAllocate"]
  },
  {
    "name": "Auditor",
    "description": "Read-only access",
    "privileges": ["VmAudit", "DatastoreAudit", "SysAudit"]
  }
]
```

### Permissions

#### Get User Permissions

Gets permissions for a specific user.

```http
GET /api/permissions/{user_id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "user_id": 2,
  "permissions": [
    {
      "path": "/api/vms/*",
      "privileges": ["VmPowerMgmt", "VmAudit"]
    }
  ]
}
```

#### Add Permission

Adds a permission to a user.

```http
POST /api/permissions/{user_id}
Authorization: Bearer <token>
Content-Type: application/json

{
  "path": "/api/vms/100",
  "privilege": "VmPowerMgmt"
}
```

**Response**:
```json
{
  "message": "Permission added successfully"
}
```

### TLS Configuration

#### Get TLS Config

Gets the current TLS configuration.

```http
GET /api/tls/config
Authorization: Bearer <token>
```

**Response**:
```json
{
  "enabled": true,
  "cert_path": "/etc/horcrux/cert.pem",
  "key_path": "/etc/horcrux/key.pem",
  "expires_at": "2026-10-09T10:30:45Z"
}
```

#### Update TLS Config

Updates TLS configuration.

```http
POST /api/tls/config
Authorization: Bearer <token>
Content-Type: application/json

{
  "enabled": true,
  "cert_path": "/etc/horcrux/cert.pem",
  "key_path": "/etc/horcrux/key.pem"
}
```

#### List Certificates

Lists all TLS certificates.

```http
GET /api/tls/certificates
Authorization: Bearer <token>
```

#### Generate Self-Signed Certificate

Generates a self-signed TLS certificate.

```http
POST /api/tls/certificate/generate
Authorization: Bearer <token>
Content-Type: application/json

{
  "common_name": "horcrux.example.com",
  "validity_days": 365
}
```

### Vault Integration

#### Get Vault Config

Gets the HashiCorp Vault configuration.

```http
GET /api/vault/config
Authorization: Bearer <token>
```

**Response**:
```json
{
  "enabled": true,
  "address": "https://vault.example.com:8200",
  "namespace": "horcrux"
}
```

#### Update Vault Config

Updates Vault configuration.

```http
POST /api/vault/config
Authorization: Bearer <token>
Content-Type: application/json

{
  "enabled": true,
  "address": "https://vault.example.com:8200",
  "token": "hvs.CAESIJ...",
  "namespace": "horcrux"
}
```

#### Read Vault Secret

Reads a secret from Vault.

```http
GET /api/vault/secret/{path}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "path": "horcrux/database",
  "data": {
    "username": "admin",
    "password": "secret123"
  }
}
```

#### Write Vault Secret

Writes a secret to Vault.

```http
POST /api/vault/secret/{path}
Authorization: Bearer <token>
Content-Type: application/json

{
  "username": "admin",
  "password": "secret123"
}
```

#### Delete Vault Secret

Deletes a secret from Vault.

```http
DELETE /api/vault/secret/{path}
Authorization: Bearer <token>
```

#### List Vault Secrets

Lists secrets at a path.

```http
GET /api/vault/secrets/{path}
Authorization: Bearer <token>
```

---

## Webhooks

### List Webhooks

Lists all webhook configurations with their delivery counters
(`success_count`, `failure_count`, `last_triggered`). The `secret` and every
header value come back as `"********"`; send that placeholder back on update
to keep the stored value.

```http
GET /api/webhooks
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "webhook-1",
    "name": "Slack Notifications",
    "url": "https://hooks.slack.com/services/...",
    "events": ["vm.started", "vm.stopped", "vm.failed"],
    "enabled": true,
    "created_at": "2025-10-01T10:00:00Z"
  }
]
```

### Create Webhook

Creates a new webhook.

```http
POST /api/webhooks
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "Slack Notifications",
  "url": "https://hooks.slack.com/services/...",
  "events": ["vm.started", "vm.stopped"],
  "enabled": true,
  "secret": "webhook_secret_key"
}
```

**Available Events**:
- `vm.created`, `vm.started`, `vm.stopped`, `vm.deleted`, `vm.failed`
- `backup.started`, `backup.completed`, `backup.failed`
- `migration.started`, `migration.completed`, `migration.failed`
- `alert.triggered`, `alert.resolved`

**Response**:
```json
{
  "id": "webhook-1",
  "message": "Webhook created successfully"
}
```

### Get Webhook

Gets details for a specific webhook.

```http
GET /api/webhooks/{id}
Authorization: Bearer <token>
```

### Update Webhook

Updates a webhook configuration.

```http
POST /api/webhooks/{id}
Authorization: Bearer <token>
Content-Type: application/json

{
  "enabled": false
}
```

### Delete Webhook

Deletes a webhook.

```http
DELETE /api/webhooks/{id}
Authorization: Bearer <token>
```

### Test Webhook

Sends one `test` event to this webhook (no retries, also when it is
disabled) and returns the delivery record. 404 if the webhook doesn't exist.

```http
POST /api/webhooks/{id}/test
Authorization: Bearer <token>
```

**Response**:
```json
{
  "webhook_id": "webhook-1",
  "event_type": "test",
  "timestamp": "2025-10-09T10:30:45Z",
  "success": true,
  "status_code": 200,
  "error": null,
  "retry_count": 0
}
```

### Get Webhook Deliveries

Gets delivery history for a webhook.

```http
GET /api/webhooks/{id}/deliveries
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "delivery-1",
    "webhook_id": "webhook-1",
    "event": "vm.started",
    "status": "success",
    "response_code": 200,
    "delivered_at": "2025-10-09T10:30:45Z",
    "retries": 0
  }
]
```

---

## Observability

### OpenTelemetry Configuration

#### Get OTel Config

Gets the OpenTelemetry configuration.

```http
GET /api/observability/config
Authorization: Bearer <token>
```

**Response**:
```json
{
  "enabled": true,
  "endpoint": "http://otel-collector:4317",
  "service_name": "horcrux-api",
  "trace_enabled": true,
  "metrics_enabled": true,
  "logs_enabled": true
}
```

#### Update OTel Config

Updates OpenTelemetry configuration.

```http
POST /api/observability/config
Authorization: Bearer <token>
Content-Type: application/json

{
  "enabled": true,
  "endpoint": "http://otel-collector:4317",
  "trace_enabled": true
}
```

#### Export Metrics Now

Triggers immediate metrics export.

```http
POST /api/observability/export/metrics
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Metrics exported successfully",
  "metrics_count": 45
}
```

### Alerts

Alert rules are evaluated by the server every 15 s against the node's
metrics (target = the node's hostname). A rule fires once its condition has
held for `duration_seconds`, is delivered to every enabled channel under
Monitoring > Notifications (`/api/notifications/channels`), and resolves on
the first sample where it no longer holds. The web UI page is
Monitoring > Alert Center (`/monitoring/alerts`).

#### List Alert Rules

```http
GET /api/alerts/rules
Authorization: Bearer ***
```

**Response**:
```json
[
  {
    "id": "rule-1",
    "name": "High CPU Usage",
    "description": "",
    "severity": "warning",
    "enabled": true,
    "condition": {
      "metric_type": "cpu_usage",
      "operator": "greaterthan",
      "threshold": 90.0,
      "target_pattern": "*",
      "duration_seconds": 300
    }
  }
]
```

- `severity`: `info`, `warning`, `critical`
- `condition.metric_type`: `cpu_usage`, `memory_usage`, `disk_usage` (data
  directory), `node_load` (1-minute load average), `vm_count` - evaluated;
  `disk_io` and `network_io` are accepted but have no node-level source yet
- `condition.operator`: `greaterthan`, `lessthan`, `equal`, `notequal`
- `condition.target_pattern`: `*`, a prefix such as `node-*`, or an exact name

#### Create Alert Rule

```http
POST /api/alerts/rules
Authorization: Bearer ***
Content-Type: application/json

{ "id": "rule-2", "name": "High Memory Usage", "description": "",
  "severity": "warning", "enabled": true,
  "condition": { "metric_type": "memory_usage", "operator": "greaterthan",
                 "threshold": 90.0, "target_pattern": "*", "duration_seconds": 300 } }
```

**Response**: `201 Created` (no body).

#### Update / Enable-Disable / Test / Delete an Alert Rule

```http
PUT  /api/alerts/rules/{rule_id}          (body: the full rule; returns it)
PUT  /api/alerts/rules/{rule_id}/toggle   {"enabled": false}  (returns the rule)
POST /api/alerts/rules/{rule_id}/test     -> {"channels": 1, "delivered": 1, "failed": 0}
DELETE /api/alerts/rules/{rule_id}        -> 204 (404 when it doesn't exist)
```

Updating, disabling or deleting a rule resolves its active alerts. `test`
sends a test alert for the rule to every enabled notification channel.

#### List Active Alerts / Alert History

```http
GET /api/alerts/active
GET /api/alerts/history?limit=100
Authorization: Bearer ***
```

**Response** (history is newest first and includes resolved alerts):
```json
[
  {
    "id": "6f1c...",
    "rule_id": "rule-1",
    "rule_name": "High CPU Usage",
    "severity": "warning",
    "status": "firing",
    "message": "High CPU Usage - CPU usage > threshold 90 (current: 97.5)",
    "target": "horcrux-node-1",
    "metric_value": 97.5,
    "threshold": 90.0,
    "fired_at": 1760700000,
    "resolved_at": null,
    "acknowledged_at": null,
    "acknowledged_by": null
  }
]
```

`status`: `firing`, `acknowledged`, `resolved`. Times are unix seconds.

#### Acknowledge Alert

```http
POST /api/alerts/{rule_id}/{target}/acknowledge
Authorization: Bearer ***
Content-Type: application/json

{"user": "admin"}
```

**Response**: `200 OK`. An acknowledged alert stays active (and is not
re-notified) until its condition clears.

#### List Notification Channels

Lists the legacy alert-manager channels (Monitoring > Notifications uses
`/api/notifications/channels`, below). The SMTP password, webhook
`auth_token` and webhook header values come back as `"********"`.

```http
GET /api/alerts/notifications
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "channel-1",
    "name": "slack",
    "type": "slack",
    "config": {
      "webhook_url": "https://hooks.slack.com/..."
    },
    "enabled": true
  }
]
```

`GET /api/notifications/channels` (and `/:id`) needs only Sys.Audit, so it
never returns a credential: e-mail passwords, webhook auth secrets and header
values, and PagerDuty integration/routing keys come back as `"********"`;
Slack, Teams and Discord webhook URLs (the URL is the posting credential)
and generic webhook URLs (receivers often take a token in the path, query or
`user:pass@`) keep only scheme, host and port, e.g.
`"https://hooks.slack.com/********"`. The same applies to the URLs returned
by `GET /api/webhooks` and `GET /api/alerts/notifications`.
`PUT /api/notifications/channels/:id` keeps the stored value for any field
sent back masked, so an edit form can re-send what it was given; a masked
header the stored channel doesn't have is dropped. A masked URL with nothing
to restore it from (a create, or a different host) is refused.

#### Add Notification Channel

Adds a new notification channel.

```http
POST /api/alerts/notifications
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "email",
  "type": "email",
  "config": {
    "smtp_server": "smtp.example.com",
    "from": "alerts@example.com",
    "to": "admin@example.com"
  }
}
```

---

## Audit Logging

### Query Audit Events

Queries the audit log.

```http
GET /api/audit/events
Authorization: Bearer <token>
```

**Query Parameters**:
- `start` - Start timestamp (ISO 8601)
- `end` - End timestamp (ISO 8601)
- `user` - Filter by username
- `action` - Filter by action (e.g., `vm.create`, `user.login`)
- `resource` - Filter by resource ID
- `limit` - Maximum results (default: 100)
- `offset` - Pagination offset

**Response**:
```json
[
  {
    "id": "audit-12345",
    "timestamp": "2025-10-09T10:30:45Z",
    "user": "admin",
    "action": "vm.start",
    "resource": "vm-100",
    "result": "success",
    "ip_address": "192.168.1.50",
    "user_agent": "curl/7.68.0"
  }
]
```

### Get Failed Logins

Gets failed login attempts.

```http
GET /api/audit/failed-logins
Authorization: Bearer <token>
```

**Query Parameters**:
- `hours` - Number of hours to look back (default: 24)

**Response**:
```json
[
  {
    "timestamp": "2025-10-09T10:25:30Z",
    "username": "admin",
    "ip_address": "192.168.1.100",
    "reason": "invalid_password"
  }
]
```

### Get Security Events

Gets security-related events.

```http
GET /api/audit/security-events
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "timestamp": "2025-10-09T10:20:15Z",
    "event_type": "unauthorized_access",
    "user": "guest",
    "resource": "/api/vms/100",
    "ip_address": "192.168.1.200"
  }
]
```

### Export Audit Logs

Exports audit logs to a file.

```http
POST /api/audit/export
Authorization: Bearer <token>
Content-Type: application/json

{
  "start": "2025-10-01T00:00:00Z",
  "end": "2025-10-09T23:59:59Z",
  "format": "json"
}
```

**Formats**: `json`, `csv`

**Response**:
```json
{
  "file_path": "/var/log/horcrux/audit-export-20251009.json",
  "events_count": 1523,
  "size_bytes": 524288
}
```

---

## Health Check

### Health Check

Checks if the API is running.

```http
GET /api/health
```

**No authentication required.**

**Response**:
```json
{
  "status": "healthy",
  "version": "1.0.0",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

---

## Error Responses

All error responses follow a standard format. See [API_ERRORS.md](./API_ERRORS.md) for complete documentation.

### Standard Error Format

```json
{
  "status": 404,
  "error": "NOT_FOUND",
  "message": "Virtual machine 'vm-100' not found",
  "details": "The requested VM does not exist in the database",
  "request_id": "req_abc123xyz",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

### Common Error Codes

| Status | Error Code | Description |
|--------|-----------|-------------|
| 400 | BAD_REQUEST | Invalid request format or parameters |
| 401 | AUTHENTICATION_FAILED | Invalid or missing authentication |
| 403 | FORBIDDEN | Insufficient permissions |
| 404 | NOT_FOUND | Resource does not exist |
| 409 | CONFLICT | Resource conflict (e.g., already exists) |
| 422 | VALIDATION_ERROR | Request validation failed |
| 429 | RATE_LIMITED | Too many requests |
| 500 | INTERNAL_ERROR | Internal server error |
| 503 | SERVICE_UNAVAILABLE | Service temporarily unavailable |

---

## Rate Limiting

The API implements rate limiting to prevent abuse:

- **Authentication endpoints**: 5 requests per minute per IP
- **General endpoints**: 100 requests per minute per user
- **Prometheus metrics**: No rate limit

Rate limit headers are included in responses:
```
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 95
X-RateLimit-Reset: 1696851600
```

---

## Pagination

List endpoints support pagination:

```http
GET /api/vms?limit=50&offset=0
```

Response includes pagination metadata:
```json
{
  "data": [...],
  "pagination": {
    "total": 150,
    "limit": 50,
    "offset": 0,
    "has_more": true
  }
}
```

---

## Versioning

The API version is included in all responses via the `X-API-Version` header:
```
X-API-Version: 1.0
```

---

## SDKs and Examples

For code examples in different languages, see:
- [Python Examples](./examples/python/)
- [Go Examples](./examples/go/)
- [TypeScript Examples](./examples/typescript/)
- [Rust Examples](./examples/rust/)

---

**Last Updated**: 2025-10-09
**API Version**: 1.0
