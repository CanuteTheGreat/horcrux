# Missing Features Compared to Proxmox VE

This document tracks features that Proxmox VE has that Horcrux doesn't yet implement.

## Priority 1: Essential Features

### 1. Backup System (vzdump equivalent)
**Status:** Feature flag exists, not implemented

Proxmox has:
- Integrated backup tool (vzdump)
- Live backup for running VMs/containers
- Scheduled backup jobs
- Full backups with compression
- Backup to local storage, NFS, CIFS, PBS
- Single-file restore from backups
- Proxmox Backup Server integration with deduplication

**What we need:**
```rust
// horcrux-api/src/backup/mod.rs
- Snapshot-based backups (using ZFS/Ceph/LVM snapshots)
- Live VM backup without downtime
- Backup scheduling (cron-like)
- Incremental backups
- Backup retention policies
- Restore functionality
```

### 2. Firewall
**Status:** Not implemented

Proxmox has:
- Distributed firewall across cluster
- Datacenter-level rules
- Node-level rules
- VM/Container-level rules
- Security groups (reusable rulesets)
- IPv4 and IPv6 support
- Logging and rate limiting

**What we need:**
```rust
// horcrux-api/src/firewall/mod.rs
- iptables/nftables integration
- Rule management per VM/container
- Security group templates
- Macro support (common rule patterns)
```

### 3. User Management & Authentication
**Status:** Not implemented

Proxmox has:
- Role-based access control (RBAC)
- Multiple authentication realms:
  - Linux PAM
  - LDAP
  - Active Directory
  - OpenID Connect
- Fine-grained permissions
- API tokens
- Two-factor authentication (TOTP, U2F)

**What we need:**
```rust
// horcrux-api/src/auth/mod.rs
- User database
- Role system (Administrator, Operator, User, etc.)
- Permission model (pools, VMs, storage, etc.)
- Session management
- API token generation
```

### 4. Cloud-Init Integration
**Status:** Not implemented

Proxmox has:
- Cloud-init drive attachment
- User/password configuration
- SSH key injection
- Network configuration
- Custom cloud-init data

**What we need:**
```rust
// horcrux-api/src/cloudinit/mod.rs
- Generate cloud-init ISO
- Attach to VMs
- Configure user-data, meta-data, network-config
- Template support
```

### 5. VM Templates
**Status:** Not implemented

Proxmox has:
- Convert VM to template
- Clone from template
- Linked clones (fast COW clones)
- Template storage

**What we need:**
```rust
// horcrux-api/src/templates/mod.rs
- Mark VM as template
- Template cloning
- Linked clone support (using ZFS/Ceph clones)
```

## Priority 2: Important Features

### 6. Console Access
**Status:** Not implemented

Proxmox has:
- noVNC (HTML5 VNC in browser)
- SPICE protocol support
- Serial console
- xterm.js for container console

**What we need:**
```rust
// horcrux-api/src/console/mod.rs
- VNC proxy server
- WebSocket proxy for console
- SPICE protocol support
```

### 7. Software-Defined Networking (SDN)
**Status:** Not implemented

Proxmox has:
- SDN zones (Simple, VLAN, VXLAN, EVPN)
- VNets (virtual networks)
- Subnets with IPAM
- BGP/EVPN routing
- VXLAN overlay networks
- Fabrics (network underlay management)

**What we need:**
```rust
// horcrux-api/src/sdn/mod.rs
- Zone management
- VNet creation
- VXLAN setup
- IPAM (IP Address Management)
- Integration with Linux bridges/OVS
```

### 8. Monitoring & Metrics
**Status:** Not implemented

Proxmox has:
- Real-time resource usage graphs
- Historical statistics (RRD-based)
- CPU, memory, disk, network graphs
- Syslog integration
- Email alerts

**What we need:**
```rust
// horcrux-api/src/monitoring/mod.rs
- Metrics collection (CPU, RAM, disk, network)
- Time-series database (Prometheus or similar)
- Alert rules
- Notification system (email, webhooks)
```

### 9. Resource Pools
**Status:** Not implemented

Proxmox has:
- Logical grouping of VMs/containers
- Permission delegation per pool
- Resource sharing/quotas

**What we need:**
```rust
// horcrux-api/src/pools/mod.rs
- Pool creation
- VM/container assignment to pools
- Permission scoping
```

### 10. Disk Management
**Status:** Partial (storage backends exist)

Proxmox has:
- Hot-plug disks
- Disk resize (online)
- Disk move between storage
- Disk import/export
- Unused disk cleanup
- Multiple disk controllers (IDE, SATA, SCSI, VirtIO)

**What we need:**
```rust
// Enhance existing VM module
- Hot-add/remove disks
- Online disk resize
- Disk migration between storage pools
```

## Priority 3: Advanced Features

### 11. Network Configuration UI
**Status:** Not implemented

Proxmox has:
- Bridge creation/management
- Bond/Team interfaces
- VLAN tagging
- OVS (Open vSwitch) support
- Network reload without reboot

**What we need:**
```rust
// horcrux-api/src/network/mod.rs
- Network interface management
- Bridge configuration
- VLAN support
- Bond/aggregation
```

### 12. ISO/Template Management
**Status:** Not implemented

Proxmox has:
- ISO library upload
- Download from URL
- Template/appliance download
- Content library

**What we need:**
```rust
// horcrux-api/src/iso/mod.rs
- ISO upload API
- Storage in designated directory
- ISO library listing
```

### 13. Task Management
**Status:** Not implemented

Proxmox has:
- Background task queue
- Task history/logs
- Task status monitoring
- Task cancellation

**What we need:**
```rust
// horcrux-api/src/tasks/mod.rs
- Async task queue
- Task tracking
- Progress reporting
- Log storage
```

### 14. HA Configuration
**Status:** Basic clustering exists, HA not implemented

Proxmox has:
- HA groups
- HA resource priorities
- Fencing configuration
- Watchdog support
- HA simulator for testing

**What we need:**
```rust
// Enhance existing cluster module
- Pacemaker resource configuration
- Fencing agents (IPMI, etc.)
- HA group management
- Watchdog integration
```

### 15. Migration Enhancements
**Status:** Basic migration planned, not implemented

Proxmox has:
- Live migration wizard
- Offline migration
- Cross-cluster migration
- Migration network selection
- Bandwidth limiting

**What we need:**
```rust
// Enhance existing cluster module
- Migration wizard API
- Bandwidth control
- Progress tracking
```

### 16. Replication
**Status:** Not implemented

Proxmox has:
- Storage replication between nodes
- Scheduled replication jobs
- ZFS send/receive
- Incremental replication

**What we need:**
```rust
// horcrux-api/src/replication/mod.rs
- ZFS send/receive jobs
- Ceph RBD mirroring
- Replication scheduling
```

### 17. PCI Passthrough
**Status:** Not implemented

Proxmox has:
- GPU passthrough
- PCIe device passthrough
- USB passthrough
- IOMMU group management

**What we need:**
```rust
// Enhance VM module
- PCI device detection
- IOMMU configuration
- Device assignment to VMs
```

### 18. Advanced VM Options
**Status:** Basic VM config exists

Proxmox has:
- CPU type selection
- NUMA configuration
- Machine type (i440fx, q35)
- BIOS/UEFI selection
- Hotplug options
- Boot order customization
- Tablet/USB input devices
- Audio device passthrough

**What we need:**
```rust
// Enhance VmConfig in horcrux-common
- Extended VM configuration options
- Hardware emulation settings
```

## Priority 4: Nice-to-Have

### 19. Subscription/Update Management
**Status:** Not applicable (different model)

Proxmox has:
- Subscription key management
- Repository configuration
- Update checking
- Package upgrade UI

**Horcrux approach:**
- Gentoo package management (emerge)
- No subscription needed (open source)
- System updates via Portage

### 20. Ceph Management
**Status:** Ceph storage backend exists, no management UI

Proxmox has:
- Ceph cluster setup wizard
- OSD management
- Pool creation
- CephFS support
- Ceph monitoring

**What we need:**
```rust
// horcrux-api/src/ceph/management.rs
- Ceph cluster deployment
- OSD management
- Pool management UI
- Health monitoring
```

### 21. Notifications
**Status:** Not implemented

Proxmox has:
- Email notifications
- Gotify support
- Sendmail/SMTP
- Custom notification targets

**What we need:**
```rust
// horcrux-api/src/notifications/mod.rs
- Email sending
- Webhook support
- Notification rules
```

### 22. Audit Log
**Status:** Not implemented

Proxmox has:
- Complete action logging
- User action tracking
- Searchable logs

**What we need:**
```rust
// horcrux-api/src/audit/mod.rs
- Action logging
- Log storage and search
```

## Implementation Priority

**Phase 1 (MVP):**
1. User Management & Authentication
2. Firewall
3. Cloud-Init
4. VM Templates
5. Console Access

**Phase 2 (Production Ready):**
6. Backup System
7. Monitoring & Metrics
8. SDN Basic (VLANs, bridges)
9. Disk Management enhancements
10. Task Management

**Phase 3 (Enterprise):**
11. HA enhancements
12. Replication
13. Advanced SDN (VXLAN, EVPN)
14. PCI Passthrough
15. Resource Pools

**Phase 4 (Polish):**
16. Migration enhancements
17. Ceph Management UI
18. Notifications
19. Audit Log
20. Advanced VM options
