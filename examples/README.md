# Horcrux Configuration Examples

This directory contains example configurations and scripts for common Horcrux deployment scenarios.

## Directory Structure

```
examples/
├── cluster/          # Cluster configuration examples
├── storage/          # Storage backend configurations
├── backup/           # Backup automation scripts
├── ha/               # High availability setup
└── README.md         # This file
```

## Cluster Examples

### Three-Node Cluster
**File:** `cluster/three-node-cluster.toml`

Production three-node cluster configuration with:
- High availability
- Shared Ceph storage
- Dedicated storage network
- Automatic failover

**Usage:**
```bash
# Copy to each node and customize
sudo cp cluster/three-node-cluster.toml /etc/horcrux/config.toml
# Edit node-specific settings (node_id, bind_address)
sudo vi /etc/horcrux/config.toml
sudo systemctl restart horcrux
```

## Storage Examples

### ZFS Storage
**File:** `storage/zfs-storage.toml`

ZFS configuration optimized for VM workloads with compression and snapshots.

**Setup:**
```bash
# Create ZFS pool
sudo zpool create -o ashift=12 tank /dev/sdb

# Apply optimizations
sudo zfs create tank/horcrux
sudo zfs set compression=lz4 tank/horcrux
sudo zfs set atime=off tank/horcrux
sudo zfs set recordsize=64K tank/horcrux

# Add to Horcrux
horcrux-cli storage create --name tank --type zfs --path tank/horcrux
```

### Ceph Storage
**File:** `storage/ceph-storage.toml`

Ceph RBD configuration for distributed storage in clusters.

**Setup:**
```bash
# Create Ceph pool
sudo ceph osd pool create horcrux 128 128
sudo ceph osd pool application enable horcrux rbd

# Configure pool for VMs
sudo ceph osd pool set horcrux size 3
sudo ceph osd pool set horcrux min_size 2

# Add to Horcrux
horcrux-cli storage create --name ceph-prod --type ceph --path horcrux
```

## Backup Examples

### Automated Backups
**File:** `backup/automated-backup.sh`

Script to configure automated backup schedules:
- Daily incremental backups
- Weekly full backups
- Hourly backups for critical VMs
- Monthly off-site backups to S3

**Usage:**
```bash
# Review and customize the script
vi backup/automated-backup.sh

# Run to create backup schedules
./backup/automated-backup.sh

# Verify schedules
horcrux-cli backup list-schedules
```

## High Availability Examples

### HA Setup
**File:** `ha/high-availability-setup.sh`

Script to configure high availability:
- Create HA groups
- Assign VMs with priorities
- Configure failover policies
- Set up automatic recovery

**Usage:**
```bash
# Ensure cluster is configured first
horcrux-cli cluster status

# Review and customize VM names
vi ha/high-availability-setup.sh

# Run HA setup
./ha/high-availability-setup.sh

# Verify HA status
horcrux-cli ha status
```

## Additional Configuration Examples

### Single Node Configuration

**File:** Create as `/etc/horcrux/config.toml`

```toml
[server]
bind_address = "0.0.0.0:8006"
workers = 8

[database]
path = "/var/lib/horcrux/horcrux.db"

[storage]
default_pool = "local"

[vm]
default_hypervisor = "qemu"
default_cpus = 2
default_memory = 2048

[auth]
session_timeout = 7200

[logging]
level = "info"
path = "/var/log/horcrux"
```

### Development Configuration

**File:** Create as `config.toml` in project root

```toml
[server]
bind_address = "127.0.0.1:8006"
workers = 4

[database]
path = "./horcrux.db"

[storage]
default_pool = "local"

[logging]
level = "debug"
path = "./logs"

[vm]
default_hypervisor = "qemu"
```

## Network Configuration Examples

### Bridge Network Setup

```bash
# Create bridge
sudo ip link add br0 type bridge
sudo ip link set br0 up
sudo ip link set eth1 master br0
sudo ip addr add 10.10.0.1/24 dev br0

# Make persistent (Gentoo)
cat >> /etc/conf.d/net << EOF
bridge_br0="eth1"
config_br0="10.10.0.1/24"
EOF
sudo ln -s /etc/init.d/net.lo /etc/init.d/net.br0
sudo rc-update add net.br0 default
```

### Firewall Rules

```bash
# Allow Horcrux API
sudo iptables -A INPUT -p tcp --dport 8006 -j ACCEPT

# Allow cluster communication
sudo iptables -A INPUT -p udp --dport 5404:5406 -j ACCEPT

# Allow VNC consoles
sudo iptables -A INPUT -p tcp --dport 5900:5999 -j ACCEPT

# Allow VM migration
sudo iptables -A INPUT -p tcp --dport 60000:60050 -j ACCEPT

# Save rules
sudo iptables-save > /etc/iptables/rules.v4
```

## Testing Configurations

### Test Basic Setup

```bash
# Start API server
horcrux-api &

# Wait for startup
sleep 2

# Check health
curl http://localhost:8006/api/health

# Create test VM
horcrux-cli vm create \
  --name test-vm \
  --cpus 1 \
  --memory 1024 \
  --disk-size 10

# List VMs
horcrux-cli vm list
```

### Test Cluster

```bash
# On each node, verify cluster membership
horcrux-cli cluster status

# Test VM migration
horcrux-cli migrate start test-vm --target-node node2

# Verify migration
horcrux-cli migrate status test-vm
```

## Customization Tips

1. **Adjust Resource Limits**
   - Increase `workers` based on CPU cores
   - Adjust `pool_size` for concurrent connections
   - Modify `session_timeout` based on security requirements

2. **Storage Paths**
   - Change `/var/lib/horcrux` to your data directory
   - Update storage pool paths for your infrastructure
   - Adjust backup destinations

3. **Network Addresses**
   - Update IP addresses and subnets for your network
   - Modify multicast addresses if needed
   - Change ports if conflicts exist

4. **Security Settings**
   - Enable TLS for production
   - Configure strong authentication
   - Set appropriate session timeouts
   - Enable audit logging

## Troubleshooting

If examples don't work:

1. Check prerequisites are met
2. Verify paths exist and have correct permissions
3. Review logs: `sudo journalctl -u horcrux -f`
4. See [TROUBLESHOOTING.md](../TROUBLESHOOTING.md)

## Additional Resources

- [Quick Start Guide](../QUICKSTART.md)
- [Deployment Guide](../DEPLOYMENT.md)
- [Troubleshooting Guide](../TROUBLESHOOTING.md)
- [API Documentation](../docs/API.md)
- [Contributing Guide](../CONTRIBUTING.md)

---

**Need help?** Open an issue: [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
