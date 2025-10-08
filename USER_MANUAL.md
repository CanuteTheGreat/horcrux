# Horcrux User Manual

Complete guide for using Horcrux virtualization platform.

## Quick Start

### Accessing Horcrux

1. Web UI: `https://your-server:8006`
2. Mobile UI: `https://your-server:8006/mobile`
3. CLI: `horcrux-cli`
4. API: `https://your-server:8006/api`

### First Login

Default credentials (change immediately!):
- Username: `admin`
- Password: `horcrux`

---

## Creating Your First VM

### Via Web UI

1. Navigate to **Virtual Machines** → **Create**
2. Fill in details:
   - Name: `my-first-vm`
   - CPU: `4 cores`
   - Memory: `8192 MB`
   - Disk: `50 GB`
   - Architecture: `x86_64`
3. Click **Create**
4. Click **Start** to boot the VM

### Via CLI

```bash
horcrux-cli vm create my-first-vm \
  --cpu 4 \
  --memory 8192 \
  --disk 50 \
  --architecture x86_64

horcrux-cli vm start my-first-vm
```

### Via API

```bash
curl -X POST https://localhost:8006/api/vms \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "vm-100",
    "name": "my-first-vm",
    "cpu_cores": 4,
    "memory_mb": 8192,
    "disks": [{"size_gb": 50}],
    "architecture": "x86_64"
  }'
```

---

## VM Management

### Starting and Stopping VMs

```bash
# Start VM
horcrux-cli vm start vm-100

# Stop VM (graceful shutdown)
horcrux-cli vm stop vm-100

# Force stop
horcrux-cli vm stop vm-100 --force

# Restart VM
horcrux-cli vm restart vm-100
```

### Viewing VM Status

```bash
# List all VMs
horcrux-cli vm list

# Get VM details
horcrux-cli vm show vm-100

# Watch VM stats
horcrux-cli vm stats vm-100 --watch
```

### Console Access

```bash
# VNC console
horcrux-cli console vm-100

# Serial console
horcrux-cli console vm-100 --serial
```

### VM Migration

```bash
# Migrate to another node
horcrux-cli vm migrate vm-100 \
  --target-node node2

# Live migration
horcrux-cli vm migrate vm-100 \
  --target-node node2 \
  --live
```

---

## Storage Management

### Creating Storage Pools

#### ZFS
```bash
horcrux-cli storage create zfs-pool \
  --type zfs \
  --pool tank/vms
```

#### Ceph
```bash
horcrux-cli storage create ceph-pool \
  --type ceph \
  --monitors 192.168.1.10:6789 \
  --pool rbd
```

#### LVM
```bash
horcrux-cli storage create lvm-pool \
  --type lvm \
  --vg vg_vms
```

#### iSCSI
```bash
horcrux-cli storage create iscsi-pool \
  --type iscsi \
  --portal 192.168.1.100:3260 \
  --iqn iqn.2025-01.com.example:storage
```

### Managing Volumes

```bash
# Create volume
horcrux-cli storage volume create zfs-pool/vm-100-disk0 \
  --size 100

# Resize volume
horcrux-cli storage volume resize zfs-pool/vm-100-disk0 \
  --size 200

# Snapshot volume
horcrux-cli storage snapshot create zfs-pool/vm-100-disk0 \
  --name backup-2025-01

# Restore snapshot
horcrux-cli storage snapshot restore zfs-pool/vm-100-disk0@backup-2025-01
```

---

## Networking

### Creating Network Zones

```bash
# Simple VLAN zone
horcrux-cli sdn zone create zone1 \
  --type simple \
  --nodes node1,node2

# VXLAN zone
horcrux-cli sdn zone create zone2 \
  --type vxlan \
  --nodes node1,node2,node3
```

### Creating Virtual Networks

```bash
# Create VNet with VLAN
horcrux-cli sdn vnet create vnet100 \
  --zone zone1 \
  --tag 100 \
  --cidr 10.0.1.0/24 \
  --gateway 10.0.1.1

# Create VNet with VXLAN
horcrux-cli sdn vnet create vnet200 \
  --zone zone2 \
  --tag 10000 \
  --type vxlan \
  --cidr 10.0.2.0/24
```

### IP Address Management

```bash
# Allocate IP for VM
horcrux-cli sdn ip allocate vnet100 \
  --vm vm-100

# Release IP
horcrux-cli sdn ip release 10.0.1.10

# List allocations
horcrux-cli sdn ip list vnet100
```

---

## Backup and Restore

### Creating Backups

```bash
# Manual backup
horcrux-cli backup create vm-100 \
  --mode snapshot \
  --compression zstd \
  --notes "Before upgrade"

# Backup to external provider
horcrux-cli backup create vm-100 \
  --provider s3-backup \
  --compression zstd
```

### Scheduled Backups

```bash
# Create backup job
horcrux-cli backup job create daily-backup \
  --schedule "0 2 * * *" \
  --vms "vm-*" \
  --mode snapshot \
  --keep-daily 7 \
  --keep-weekly 4 \
  --keep-monthly 3
```

### Restoring Backups

```bash
# List backups
horcrux-cli backup list

# Restore to original VM
horcrux-cli backup restore backup-2025-01-01-vm-100

# Restore to new VM
horcrux-cli backup restore backup-2025-01-01-vm-100 \
  --target vm-101
```

---

## Templates

### Creating Templates

```bash
# Convert VM to template
horcrux-cli template create vm-100 \
  --name "Ubuntu 22.04 Server"

# Create template from ISO
horcrux-cli template create-from-iso \
  --iso /var/lib/iso/ubuntu-22.04.iso \
  --name "Ubuntu 22.04"
```

### Using Templates

```bash
# Full clone
horcrux-cli template clone tmpl-100 \
  --name new-vm \
  --type full

# Linked clone (faster, uses less space)
horcrux-cli template clone tmpl-100 \
  --name new-vm \
  --type linked
```

### Cloud-Init Integration

```bash
# Create VM from template with cloud-init
horcrux-cli template clone tmpl-100 \
  --name web-server \
  --cloud-init \
  --user admin \
  --ssh-key ~/.ssh/id_rsa.pub \
  --ip 10.0.1.100/24 \
  --gateway 10.0.1.1
```

---

## Clustering

### Viewing Cluster Status

```bash
# Cluster overview
horcrux-cli cluster status

# Node list
horcrux-cli cluster nodes

# Quorum status
horcrux-cli cluster quorum
```

### High Availability

```bash
# Enable HA for VM
horcrux-cli ha enable vm-100 \
  --priority 100

# Set affinity rules
horcrux-cli ha affinity vm-100 \
  --nodes node1,node2 \
  --policy required

# Anti-affinity (keep VMs apart)
horcrux-cli ha anti-affinity vm-100,vm-101
```

### Mixed-Architecture Clustering

```bash
# View architecture distribution
horcrux-cli cluster architecture

# Find best node for VM
horcrux-cli cluster find-node \
  --vm-arch riscv64 \
  --prefer-native

# Create RISC-V VM
horcrux-cli vm create riscv-vm \
  --architecture riscv64 \
  --cpu 4 \
  --memory 8192
```

---

## Monitoring

### Real-Time Monitoring

```bash
# Node stats
horcrux-cli monitoring node

# VM stats
horcrux-cli monitoring vm vm-100

# Storage stats
horcrux-cli monitoring storage zfs-pool

# Watch metrics (auto-refresh)
horcrux-cli monitoring watch
```

### Alert Rules

```bash
# Create alert
horcrux-cli alerts create high-cpu \
  --metric cpu_usage \
  --threshold 80 \
  --severity warning \
  --target "vm-*"

# List active alerts
horcrux-cli alerts list --active

# Acknowledge alert
horcrux-cli alerts ack high-cpu vm-100
```

### OpenTelemetry Export

```bash
# Configure OTLP export
horcrux-cli observability enable \
  --endpoint http://otlp-collector:4318 \
  --interval 60

# Export metrics now
horcrux-cli observability export
```

---

## Security

### Two-Factor Authentication

```bash
# Enable 2FA
horcrux-cli auth 2fa enable <username>

# Disable 2FA
horcrux-cli auth 2fa disable <username>

# Regenerate backup codes
horcrux-cli auth 2fa regen-codes <username>
```

### Firewall Management

```bash
# Create firewall rule
horcrux-cli firewall rule create \
  --action accept \
  --protocol tcp \
  --port 80 \
  --direction in

# Create security group
horcrux-cli firewall group create web-server \
  --rules "tcp/80/in,tcp/443/in"

# Apply security group to VM
horcrux-cli firewall apply vm-100 \
  --group web-server
```

### User Management

```bash
# Create user
horcrux-cli user create bob \
  --realm pam \
  --role PVEVMUser

# Assign permissions
horcrux-cli user grant bob \
  --path /vms/vm-100 \
  --role PVEVMUser

# List users
horcrux-cli user list
```

---

## Mobile UI

The mobile UI provides touch-optimized access for phones and tablets:

### Features
- Dashboard with cluster stats
- VM management (start/stop/console)
- Cluster node monitoring
- Quick actions
- Offline support (planned)

### Accessing
1. Open `https://your-server:8006/mobile` in mobile browser
2. Login with your credentials
3. Add to home screen for app-like experience

---

## Troubleshooting

### VM Won't Start

```bash
# Check VM configuration
horcrux-cli vm show vm-100

# View VM logs
horcrux-cli vm logs vm-100

# Check storage
horcrux-cli storage status

# Verify node resources
horcrux-cli monitoring node
```

### Network Issues

```bash
# Test network connectivity
horcrux-cli network test vnet100

# Check SDN status
horcrux-cli sdn status

# View IP allocations
horcrux-cli sdn ip list vnet100
```

### Cluster Problems

```bash
# Check cluster status
horcrux-cli cluster status

# Verify quorum
horcrux-cli cluster quorum

# Test node connectivity
horcrux-cli cluster ping node2

# View corosync status
corosync-cfgtool -s
```

---

## Best Practices

### VM Management
- Use templates for consistent deployments
- Enable HA for critical VMs
- Regular backups with retention policies
- Monitor resource usage
- Use cloud-init for automation

### Storage
- Use ZFS/Ceph for production
- Enable compression and deduplication
- Regular snapshots
- Monitor disk I/O
- Plan for growth

### Networking
- Use VLANs for segmentation
- VXLAN for overlay networks
- SDN fabrics for spine-leaf
- Document IP allocations
- Implement firewall rules

### Security
- Enable 2FA for all users
- Use TLS for API access
- Regular security updates
- Implement least privilege
- Monitor audit logs

### Clustering
- Use redundant network connections
- Enable HA for critical services
- Mix architectures strategically
- Set affinity rules appropriately
- Monitor cluster health

---

## Advanced Topics

### Custom Scripts

```bash
# VM lifecycle hooks
/etc/horcrux/hooks/vm-start.sh
/etc/horcrux/hooks/vm-stop.sh

# Backup hooks
/etc/horcrux/hooks/backup-pre.sh
/etc/horcrux/hooks/backup-post.sh
```

### API Integration

See [API_DOCUMENTATION.md](API_DOCUMENTATION.md) for detailed API reference.

### External Backup Providers

```bash
# S3 provider
horcrux-cli backup provider add s3 \
  --endpoint https://s3.amazonaws.com \
  --bucket backups \
  --access-key <key> \
  --secret-key <secret>

# Custom HTTP provider
horcrux-cli backup provider add custom \
  --type http \
  --endpoint https://backup.example.com \
  --auth-token <token>
```

---

## Keyboard Shortcuts (Web UI)

- `Ctrl+K`: Quick command
- `Ctrl+N`: New VM
- `Ctrl+B`: Backup
- `Ctrl+M`: Monitoring
- `/`: Search
- `?`: Help

---

## Getting Help

- Web UI: Click **Help** → **Documentation**
- CLI: `horcrux-cli help <command>`
- Community: https://community.horcrux.io
- Documentation: https://docs.horcrux.io
- GitHub Issues: https://github.com/yourusername/horcrux/issues

---

## Appendix

### CLI Command Reference

```bash
horcrux-cli
├── vm (create, start, stop, delete, migrate)
├── storage (create, list, volumes, snapshots)
├── backup (create, restore, jobs, providers)
├── cluster (status, nodes, join, quorum)
├── sdn (zones, vnets, subnets, ip)
├── firewall (rules, groups, apply)
├── template (create, clone, delete)
├── monitoring (node, vms, storage)
├── alerts (create, list, ack)
├── auth (login, 2fa, users)
└── observability (config, export)
```

### Architecture Support Matrix

| Architecture | QEMU | LXC | Status |
|--------------|------|-----|--------|
| x86_64 | ✅ | ✅ | Full Support |
| aarch64 | ✅ | ✅ | Full Support |
| riscv64 | ✅ | ⚠️ | Experimental |
| ppc64le | ✅ | ⚠️ | Experimental |
| s390x | ✅ | ❌ | Limited |
| mips64 | ✅ | ❌ | Limited |

---

Happy virtualizing! 🎉
