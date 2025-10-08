# Horcrux Features

A comprehensive Gentoo-native virtualization management platform.

## Complete Feature Matrix

### Virtualization Hypervisors
| Backend | USE Flag | Description | VM Support | Container Support |
|---------|----------|-------------|------------|-------------------|
| **QEMU/KVM** | `qemu` ✓ | Hardware-accelerated VMs | ✅ | ❌ |
| **LXD** | `lxd` | Modern unified management | ✅ | ✅ |
| **Incus** | `incus` | LXD fork with active development | ✅ | ✅ |

### Container Runtimes
| Runtime | USE Flag | Description | Daemon | OCI Compatible |
|---------|----------|-------------|--------|----------------|
| **LXC** | `lxc` ✓ | System containers | No | No |
| **LXD** | `lxd` | LXD containers (same backend) | Yes | No |
| **Incus** | `incus` | Incus containers (same backend) | Yes | No |
| **Docker** | `docker` ✓ | Industry standard | Yes | Yes |
| **Podman** | `podman` | Daemonless alternative | No | Yes |

✓ = Enabled by default

### Storage Backends
| Backend | USE Flag | Features | Snapshots | Clones | Distributed |
|---------|----------|----------|-----------|--------|-------------|
| **ZFS** | `zfs` | Copy-on-write filesystem | ✅ | ✅ | ❌ |
| **Ceph RBD** | `ceph` | Distributed block storage | ✅ | ✅ | ✅ |
| **LVM** | `lvm` | Logical Volume Manager | ✅ | ❌ | ❌ |
| **Directory** | - | File-based (qcow2) | ❌ | ❌ | ❌ |

### Clustering Features
| Feature | USE Flag | Description |
|---------|----------|-------------|
| **Multi-node** | `cluster` | Multiple servers in one cluster |
| **Quorum** | `cluster` | Corosync-based quorum |
| **HA (High Availability)** | `cluster` | Automatic VM failover |
| **Live Migration** | `cluster` | Move VMs between nodes without downtime |
| **Shared Configuration** | `cluster` | Cluster-wide VM/container configs |

### Additional Features
| Feature | USE Flag | Description |
|---------|----------|-------------|
| **Web UI** | `webui` ✓ | Modern Rust/WASM interface |
| **REST API** | - | Full-featured API (always included) |
| **Backup/Restore** | `backup` | VM and container backups |
| **LDAP Auth** | `ldap` | Enterprise authentication |
| **SSL/TLS** | `ssl` | Secure HTTPS communication |
| **IPv6** | `ipv6` | IPv6 networking support |

## Clustering Architecture

### Components
- **Corosync**: Cluster communication layer and membership
- **Pacemaker**: Resource management and HA orchestration
- **Distributed Locks**: Prevent conflicting operations across nodes
- **Quorum**: Ensures cluster decisions are made safely

### Cluster Operations
```bash
# Initialize cluster
POST /api/cluster/create

# Join existing cluster
POST /api/cluster/join

# Add/remove nodes
POST /api/cluster/nodes
DELETE /api/cluster/nodes/{id}

# Migrate VM
POST /api/vms/{id}/migrate

# Enable HA for VM
POST /api/vms/{id}/ha/enable
```

## Storage Architecture

### Supported Operations
- **Volume Creation**: Create block devices for VMs
- **Snapshots**: Point-in-time copies (ZFS, Ceph, LVM)
- **Cloning**: Fast COW clones (ZFS, Ceph)
- **Live Resizing**: Grow volumes on the fly
- **Thin Provisioning**: Overcommit storage

### Storage Pools
```bash
# Add ZFS pool
POST /api/storage/pools
{
  "type": "zfs",
  "name": "tank/vms",
  "path": "tank"
}

# Add Ceph pool
POST /api/storage/pools
{
  "type": "ceph",
  "name": "rbd",
  "path": "rbd"
}

# Create volume
POST /api/storage/pools/{id}/volumes
{
  "name": "vm-100-disk-0",
  "size_gb": 32
}

# Snapshot
POST /api/storage/pools/{id}/volumes/{name}/snapshots
{
  "snapshot_name": "before-upgrade"
}
```

## Use Case Examples

### 1. Home Lab (Minimal)
```bash
USE="qemu lxc webui" emerge horcrux
```
- QEMU/KVM for VMs
- LXC for containers
- Web UI for management
- Single node, no clustering

### 2. Development Environment
```bash
USE="qemu docker podman webui zfs backup" emerge horcrux
```
- Multiple container runtimes for testing
- ZFS for fast snapshots
- Backup support for saving states

### 3. Small Business (HA)
```bash
USE="qemu lxc cluster webui zfs ssl ldap" emerge horcrux
```
- High availability clustering
- Enterprise authentication
- Secure HTTPS
- ZFS for snapshots

### 4. Enterprise Data Center
```bash
USE="qemu lxd incus docker cluster ceph ssl ldap backup webui" emerge horcrux
```
- All hypervisors and runtimes
- Ceph distributed storage
- Full clustering with HA
- LDAP integration
- Complete backup solution

### 5. Ceph-based Cluster
```bash
USE="qemu cluster ceph ssl webui" emerge horcrux
```
- QEMU VMs only
- Ceph for shared storage across cluster
- Live migration between nodes
- Distributed, redundant storage

### 6. Container-Only Platform
```bash
USE="lxc docker podman webui" emerge horcrux
```
- No VM support
- Multiple container runtimes
- Lightweight deployment

## API Endpoints

### Virtual Machines
- `GET /api/vms` - List all VMs
- `POST /api/vms` - Create VM
- `GET /api/vms/{id}` - Get VM details
- `POST /api/vms/{id}/start` - Start VM
- `POST /api/vms/{id}/stop` - Stop VM
- `DELETE /api/vms/{id}` - Delete VM
- `POST /api/vms/{id}/migrate` - Migrate to another node

### Containers
- `GET /api/containers` - List all containers
- `POST /api/containers` - Create container
- `GET /api/containers/{id}` - Get container details
- `POST /api/containers/{id}/start` - Start container
- `POST /api/containers/{id}/stop` - Stop container
- `DELETE /api/containers/{id}` - Delete container

### Storage
- `GET /api/storage/pools` - List storage pools
- `POST /api/storage/pools` - Add storage pool
- `POST /api/storage/pools/{id}/volumes` - Create volume
- `POST /api/storage/pools/{id}/volumes/{name}/snapshots` - Create snapshot
- `POST /api/storage/pools/{id}/volumes/{name}/restore` - Restore snapshot

### Cluster
- `POST /api/cluster/create` - Initialize cluster
- `POST /api/cluster/join` - Join cluster
- `GET /api/cluster/nodes` - List cluster nodes
- `POST /api/cluster/nodes` - Add node
- `DELETE /api/cluster/nodes/{id}` - Remove node
- `GET /api/cluster/status` - Get cluster status

## Comparison to Proxmox VE

| Feature | Proxmox VE | Horcrux |
|---------|------------|---------|
| **Base OS** | Debian | Gentoo |
| **Hypervisors** | QEMU/KVM, LXC | QEMU/KVM, LXC, LXD, Incus |
| **Containers** | LXC | LXC, LXD, Incus, Docker, Podman |
| **Storage** | ZFS, Ceph, LVM, Dir | ZFS, Ceph, LVM, Dir |
| **Clustering** | Corosync/Pacemaker | Corosync/Pacemaker |
| **Language** | Perl, JavaScript | **Rust** |
| **Customization** | Limited | **USE flags for everything** |
| **Package Management** | apt | emerge (source-based) |
| **Optimization** | Generic binaries | **Compiled for your hardware** |

## Why Horcrux on Gentoo?

1. **Fine-grained control**: Enable only what you need via USE flags
2. **Optimized builds**: Compiled specifically for your hardware
3. **Rolling release**: Always up-to-date with latest features
4. **Source-based**: Full transparency and customization
5. **Rust-native**: Memory safety, performance, modern codebase
6. **Multiple backends**: More choice than Proxmox (LXD, Incus, Podman)
