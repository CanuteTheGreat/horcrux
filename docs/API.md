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
- `422 VALIDATION_ERROR` - Invalid configuration

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
- `id` (optional) - VM ID for the clone (auto-generated if not provided)
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

### List Backups

Lists all backups.

```http
GET /api/backups
Authorization: Bearer <token>
```

**Query Parameters**:
- `vm_id` (optional) - Filter by VM ID
- `status` (optional) - Filter by status: `pending`, `running`, `completed`, `failed`

**Response**:
```json
[
  {
    "id": "backup-20251009-103045",
    "vm_id": "100",
    "vm_name": "web-server-01",
    "type": "full",
    "status": "completed",
    "size": 5368709120,
    "path": "/var/backups/horcrux/vm-100/backup-20251009-103045.tar.gz",
    "created_at": "2025-10-09T10:30:45Z",
    "completed_at": "2025-10-09T10:45:12Z",
    "compression": "gzip"
  }
]
```

### Create Backup

Creates a backup of a VM.

```http
POST /api/backups
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "100",
  "type": "full",
  "compression": "gzip",
  "description": "Weekly backup"
}
```

**Parameters**:
- `vm_id` (required) - VM to backup
- `type` (optional) - Backup type: `full` or `incremental` (default: `full`)
- `compression` (optional) - Compression: `gzip`, `bzip2`, `xz`, `none` (default: `gzip`)
- `description` (optional) - Backup description

**Response**:
```json
{
  "id": "backup-20251009-103045",
  "vm_id": "100",
  "status": "running",
  "message": "Backup started"
}
```

### Get Backup

Gets details for a specific backup.

```http
GET /api/backups/{id}
Authorization: Bearer <token>
```

**Response**: Same as individual backup object in list response.

### Restore Backup

Restores a VM from a backup.

```http
POST /api/backups/{id}/restore
Authorization: Bearer <token>
Content-Type: application/json

{
  "target_vm_id": "101",
  "overwrite": false
}
```

**Parameters**:
- `target_vm_id` (optional) - Restore to different VM ID (creates new VM if doesn't exist)
- `overwrite` (optional) - Overwrite existing VM (default: false)

**Response**:
```json
{
  "vm_id": "101",
  "message": "Backup restored successfully"
}
```

### Delete Backup

Deletes a backup.

```http
DELETE /api/backups/{id}
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Backup deleted successfully"
}
```

### Backup Jobs

#### List Backup Jobs

Lists scheduled backup jobs.

```http
GET /api/backup-jobs
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "job-1",
    "name": "Daily VM Backups",
    "schedule": "0 2 * * *",
    "vm_ids": ["100", "101", "102"],
    "retention_days": 30,
    "enabled": true,
    "last_run": "2025-10-09T02:00:00Z",
    "next_run": "2025-10-10T02:00:00Z"
  }
]
```

#### Create Backup Job

Creates a scheduled backup job.

```http
POST /api/backup-jobs
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "Daily VM Backups",
  "schedule": "0 2 * * *",
  "vm_ids": ["100", "101", "102"],
  "type": "full",
  "compression": "gzip",
  "retention_days": 30,
  "enabled": true
}
```

**Schedule Format**: Cron expression (minute hour day month weekday)

**Response**:
```json
{
  "id": "job-1",
  "name": "Daily VM Backups",
  "message": "Backup job created successfully"
}
```

#### Run Backup Job Now

Triggers a backup job immediately.

```http
POST /api/backup-jobs/{id}/run
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Backup job started",
  "backups_created": 3
}
```

### Apply Retention Policy

Applies retention policy to delete old backups.

```http
POST /api/backups/retention/{target_id}
Authorization: Bearer <token>
Content-Type: application/json

{
  "retention_days": 30
}
```

**Response**:
```json
{
  "message": "Retention policy applied",
  "deleted_backups": 5
}
```

---

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
  "name": "web-server-03",
  "id": "103",
  "start": true
}
```

**Response**:
```json
{
  "vm_id": "103",
  "name": "web-server-03",
  "message": "VM created from template successfully"
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

Attaches a container to a CNI network.

```http
POST /api/cni/attach
Authorization: Bearer <token>
Content-Type: application/json

{
  "container_id": "container-123",
  "network_name": "web-network",
  "interface_name": "eth0"
}
```

**Response**:
```json
{
  "container_id": "container-123",
  "network": "web-network",
  "ip_address": "10.50.0.10",
  "gateway": "10.50.0.1"
}
```

#### Detach Container from Network

Detaches a container from a CNI network.

```http
POST /api/cni/detach
Authorization: Bearer <token>
Content-Type: application/json

{
  "container_id": "container-123",
  "network_name": "web-network",
  "interface_name": "eth0"
}
```

### Network Policies

#### List Network Policies

Lists all network policies.

```http
GET /api/network-policies
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "policy-1",
    "name": "web-tier-policy",
    "namespace": "production",
    "pod_selector": {
      "tier": "web"
    },
    "ingress": [
      {
        "from": [
          {"pod_selector": {"tier": "frontend"}}
        ],
        "ports": [
          {"protocol": "TCP", "port": 80}
        ]
      }
    ],
    "egress": [
      {
        "to": [
          {"pod_selector": {"tier": "database"}}
        ],
        "ports": [
          {"protocol": "TCP", "port": 5432}
        ]
      }
    ]
  }
]
```

#### Create Network Policy

Creates a new network policy (Kubernetes-style).

```http
POST /api/network-policies
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "web-tier-policy",
  "namespace": "production",
  "pod_selector": {
    "tier": "web"
  },
  "ingress": [
    {
      "from": [
        {"pod_selector": {"tier": "frontend"}}
      ],
      "ports": [
        {"protocol": "TCP", "port": 80}
      ]
    }
  ],
  "policy_types": ["Ingress", "Egress"]
}
```

**Response**:
```json
{
  "id": "policy-1",
  "name": "web-tier-policy",
  "message": "Network policy created successfully"
}
```

#### Get Network Policy

Gets details for a specific network policy.

```http
GET /api/network-policies/{id}
Authorization: Bearer <token>
```

#### Delete Network Policy

Deletes a network policy.

```http
DELETE /api/network-policies/{id}
Authorization: Bearer <token>
```

#### Get Policy iptables Rules

Gets the generated iptables rules for a policy.

```http
GET /api/network-policies/{id}/iptables
Authorization: Bearer <token>
```

**Response**:
```json
{
  "policy_id": "policy-1",
  "rules": [
    "iptables -A INPUT -s 10.50.0.0/24 -p tcp --dport 80 -j ACCEPT",
    "iptables -A OUTPUT -d 10.60.0.0/24 -p tcp --dport 5432 -j ACCEPT"
  ]
}
```

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

**Response**:
```json
{
  "vm_id": "100",
  "status": "running",
  "cpu_usage": 25.3,
  "memory_used": 2147483648,
  "memory_total": 4294967296,
  "memory_usage": 50.0,
  "disk_read_bytes": 1073741824,
  "disk_write_bytes": 536870912,
  "network_rx_bytes": 2147483648,
  "network_tx_bytes": 1073741824,
  "uptime_seconds": 3600
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

Lists all nodes in the cluster.

```http
GET /api/cluster/nodes
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "name": "horcrux-node-1",
    "address": "192.168.1.10",
    "architecture": "x86_64",
    "status": "online",
    "cpu_cores": 8,
    "memory_total": 17179869184,
    "vms_running": 5,
    "joined_at": "2025-10-01T10:00:00Z"
  }
]
```

### Add Cluster Node

Adds a new node to the cluster.

```http
POST /api/cluster/nodes/{name}
Authorization: Bearer <token>
Content-Type: application/json

{
  "address": "192.168.1.11",
  "architecture": "x86_64"
}
```

**Response**:
```json
{
  "name": "horcrux-node-2",
  "status": "online",
  "message": "Node added to cluster successfully"
}
```

### Get Cluster Architecture

Gets the architecture topology of the cluster.

```http
GET /api/cluster/architecture
Authorization: Bearer <token>
```

**Response**:
```json
{
  "total_nodes": 3,
  "architectures": {
    "x86_64": 2,
    "aarch64": 1
  },
  "nodes": [
    {
      "name": "horcrux-node-1",
      "architecture": "x86_64",
      "compatible_architectures": ["x86_64"]
    }
  ]
}
```

### Find Best Node for VM

Finds the optimal node for VM placement.

```http
POST /api/cluster/find-node
Authorization: Bearer <token>
Content-Type: application/json

{
  "memory": 4096,
  "cpus": 2,
  "architecture": "x86_64"
}
```

**Response**:
```json
{
  "recommended_node": "horcrux-node-2",
  "score": 85,
  "reason": "Lowest CPU usage, sufficient resources"
}
```

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

Lists all webhook configurations.

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

Sends a test event to a webhook.

```http
POST /api/webhooks/{id}/test
Authorization: Bearer <token>
```

**Response**:
```json
{
  "status": "success",
  "response_code": 200,
  "response_time_ms": 145
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

#### List Alert Rules

Lists all alert rules.

```http
GET /api/alerts/rules
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "id": "rule-1",
    "name": "High CPU Usage",
    "condition": "cpu_usage > 90",
    "duration": 300,
    "severity": "warning",
    "enabled": true,
    "notification_channels": ["slack", "email"]
  }
]
```

#### Create Alert Rule

Creates a new alert rule.

```http
POST /api/alerts/rules
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "High Memory Usage",
  "condition": "memory_usage > 90",
  "duration": 300,
  "severity": "warning",
  "notification_channels": ["slack"]
}
```

**Severities**: `info`, `warning`, `critical`

**Response**:
```json
{
  "id": "rule-2",
  "message": "Alert rule created successfully"
}
```

#### Delete Alert Rule

Deletes an alert rule.

```http
DELETE /api/alerts/rules/{rule_id}
Authorization: Bearer <token>
```

#### List Active Alerts

Lists currently active alerts.

```http
GET /api/alerts/active
Authorization: Bearer <token>
```

**Response**:
```json
[
  {
    "alert_id": "alert-1",
    "rule_id": "rule-1",
    "rule_name": "High CPU Usage",
    "target": "horcrux-node-1",
    "severity": "warning",
    "triggered_at": "2025-10-09T10:30:45Z",
    "acknowledged": false
  }
]
```

#### Get Alert History

Gets historical alerts.

```http
GET /api/alerts/history
Authorization: Bearer <token>
```

**Query Parameters**:
- `start` - Start timestamp
- `end` - End timestamp
- `severity` - Filter by severity

#### Acknowledge Alert

Acknowledges an active alert.

```http
POST /api/alerts/{rule_id}/{target}/acknowledge
Authorization: Bearer <token>
```

**Response**:
```json
{
  "message": "Alert acknowledged"
}
```

#### List Notification Channels

Lists configured notification channels.

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
