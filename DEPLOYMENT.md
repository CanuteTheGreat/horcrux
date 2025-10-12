# Horcrux Deployment Guide

Production deployment guide for single node and clustered Horcrux installations.

## Quick Deployment Options

### Single Node (Development/Small Scale)
```bash
./install.sh --prefix /usr/local --with-systemd
sudo systemctl start horcrux
```

### Clustered (Production)
See [Clustered Deployment](#clustered-deployment) section below.

## System Requirements

### Minimum (Development)
- CPU: 2 cores | RAM: 4 GB | Disk: 20 GB | Network: 100 Mbps

### Production Single Node
- CPU: 8 cores (VT-x/AMD-V) | RAM: 32 GB | Disk: 500 GB SSD | Network: 1 Gbps

### Production Cluster Node  
- CPU: 16+ cores (VT-x/AMD-V) | RAM: 64 GB+ | Disk: 1 TB NVMe | Network: 10 Gbps

## Single Node Deployment

### 1. Installation
```bash
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux
sudo ./install.sh --with-systemd
```

### 2. Configuration
Edit `/etc/horcrux/config.toml`:
```toml
[server]
bind_address = "0.0.0.0:8006"
workers = 8

[storage]
default_pool = "local"
```

### 3. Start Service
```bash
sudo systemctl enable --now horcrux
curl http://localhost:8006/api/health
```

## Clustered Deployment

### Network Planning
- Management: 192.168.1.0/24
- Storage: 10.0.0.0/24 (dedicated)

### Node Configuration
```toml
[cluster]
enabled = true
node_id = "node1"
cluster_name = "production"
bind_address = "192.168.1.101"
```

### Setup Cluster
```bash
# Node 1 (init)
horcrux-cli cluster init --node-id node1 --cluster-name production

# Nodes 2-3 (join)
horcrux-cli cluster join --node-id node2 --primary-node 192.168.1.101
```

## Storage Backend Setup

### ZFS (Recommended)
```bash
sudo zpool create tank /dev/sdb
horcrux-cli storage create --name tank --type zfs --path tank/horcrux
```

### Ceph (For Clusters)
```bash
sudo ceph osd pool create horcrux 128
horcrux-cli storage create --name ceph-pool --type ceph --path horcrux
```

### NFS
```bash
horcrux-cli storage create --name nfs-storage --type nfs --path nfs://server/path
```

## High Availability

### Install Corosync/Pacemaker
```bash
sudo apt install corosync pacemaker
```

### Create VIP Resource
```bash
sudo pcs resource create horcrux_vip IPaddr2 ip=192.168.1.100 cidr_netmask=24
sudo pcs resource create horcrux systemd:horcrux
sudo pcs constraint colocation add horcrux with horcrux_vip INFINITY
```

## Security

### Enable SSL/TLS
```bash
sudo certbot certonly --standalone -d horcrux.example.com
```

Update config:
```toml
[server]
tls_enabled = true
tls_cert = "/etc/letsencrypt/live/horcrux.example.com/fullchain.pem"
tls_key = "/etc/letsencrypt/live/horcrux.example.com/privkey.pem"
```

### Firewall Rules
```bash
sudo iptables -A INPUT -p tcp --dport 8006 -j ACCEPT  # API
sudo iptables -A INPUT -p udp --dport 5404:5406 -j ACCEPT  # Cluster
sudo iptables -A INPUT -p tcp --dport 5900:5999 -j ACCEPT  # VNC
```

## Monitoring

### Prometheus
```toml
[monitoring]
prometheus_enabled = true
prometheus_port = 9090
```

### Alerts
```bash
horcrux-cli alert create-rule --name high-cpu --metric cpu_usage --threshold 80
```

## Backup Strategy

### Automated Backups
```bash
horcrux-cli backup create-schedule \
  --name daily-backup \
  --vms "vm-*" \
  --schedule "0 2 * * *" \
  --retention 7
```

## Deployment Checklist

### Pre-Deployment
- [ ] Hardware meets requirements
- [ ] Network configured
- [ ] Storage backend selected
- [ ] Security policies reviewed

### Deployment
- [ ] Install Horcrux
- [ ] Configure cluster
- [ ] Set up storage
- [ ] Enable SSL/TLS
- [ ] Configure RBAC

### Post-Deployment
- [ ] Verify cluster status
- [ ] Test VM creation
- [ ] Configure monitoring
- [ ] Set up backups
- [ ] Test disaster recovery

## Performance Tuning

### Kernel Parameters
```bash
sudo sysctl -w vm.swappiness=10
sudo sysctl -w vm.dirty_ratio=10
```

### ZFS Optimization
```bash
sudo zfs set compression=lz4 tank/horcrux
sudo zfs set atime=off tank/horcrux
```

## Additional Resources

- [Quick Start](QUICKSTART.md)
- [Troubleshooting](TROUBLESHOOTING.md)
- [API Documentation](docs/API.md)
- [RBAC Guide](docs/RBAC.md)

---

**Need help?** [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
