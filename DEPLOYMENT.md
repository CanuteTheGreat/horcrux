# Horcrux Deployment Guide

Complete guide for deploying Horcrux in production environments.

## Table of Contents

1. [System Requirements](#system-requirements)
2. [Installation Methods](#installation-methods)
3. [Configuration](#configuration)
4. [Network Setup](#network-setup)
5. [Security Hardening](#security-hardening)
6. [Clustering](#clustering)
7. [Backup Configuration](#backup-configuration)
8. [Monitoring Setup](#monitoring-setup)

---

## System Requirements

### Minimum Requirements
- CPU: 4 cores (x86_64, aarch64, or riscv64)
- RAM: 8 GB
- Disk: 50 GB for system + storage for VMs
- Network: 1 Gbps
- OS: Gentoo Linux (kernel 6.1+)

### Recommended for Production
- CPU: 16+ cores
- RAM: 64+ GB
- Disk: NVMe SSD for system, ZFS/Ceph for VM storage
- Network: 10 Gbps, redundant NICs
- OS: Gentoo Linux with hardened profile

### Supported Architectures
- x86_64 (AMD64/Intel)
- aarch64 (ARM64)
- riscv64 (RISC-V 64-bit)
- ppc64le (PowerPC 64-bit LE)
- s390x (IBM System z)
- mips64 (MIPS 64-bit)

---

## Installation Methods

### Method 1: Gentoo Ebuild (Recommended)

```bash
# Add Horcrux overlay
eselect repository add horcrux git https://github.com/yourusername/horcrux-overlay

# Sync repositories
emaint sync -r horcrux

# Install with USE flags
echo "app-emulation/horcrux qemu lxd incus docker podman zfs ceph lvm" >> /etc/portage/package.use/horcrux

# Install
emerge app-emulation/horcrux

# Enable and start services
rc-update add horcrux default
rc-service horcrux start

# OR for systemd:
systemctl enable horcrux
systemctl start horcrux
```

### Method 2: From Source

```bash
# Prerequisites
emerge dev-lang/rust virtual/cargo

# Clone repository
git clone https://github.com/yourusername/horcrux.git
cd horcrux

# Build
cargo build --release

# Install
cargo install --path horcrux-api

# Create systemd service
cat > /etc/systemd/system/horcrux.service <<EOF
[Unit]
Description=Horcrux Virtualization Platform
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/horcrux-api
Restart=always
User=root

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable horcrux
systemctl start horcrux
```

### Method 3: Docker (Development Only)

```bash
docker run -d \
  --name horcrux \
  --privileged \
  -v /var/lib/horcrux:/var/lib/horcrux \
  -v /dev:/dev \
  -p 8006:8006 \
  horcrux/horcrux:latest
```

---

## Configuration

### Main Configuration File

`/etc/horcrux/config.toml`:

```toml
[server]
listen_address = "0.0.0.0:8006"
tls_enabled = true
tls_cert = "/etc/horcrux/ssl/cert.pem"
tls_key = "/etc/horcrux/ssl/key.pem"

[cluster]
name = "production"
node_name = "pve-node1"
corosync_enabled = true

[storage]
default_pool = "zfs-pool"
backup_dir = "/var/lib/horcrux/backups"

[auth]
realm = "pam"
session_timeout = 7200  # 2 hours

[observability]
opentelemetry_enabled = true
otlp_endpoint = "http://localhost:4318"
```

### Storage Configuration

`/etc/horcrux/storage.conf`:

```ini
[zfs-pool]
type = zfs
pool = tank/vms
content = images,rootdir

[ceph-pool]
type = ceph
monitors = 192.168.1.10:6789,192.168.1.11:6789
pool = rbd
username = admin
secret = /etc/ceph/ceph.client.admin.keyring

[lvm-pool]
type = lvm
vgname = vg_vms
```

---

## Network Setup

### Basic Networking

```bash
# Create management bridge
ip link add vmbr0 type bridge
ip link set vmbr0 up
ip addr add 192.168.1.100/24 dev vmbr0

# Add physical interface to bridge
ip link set eth0 master vmbr0
```

### SDN Configuration

```bash
# Configure VXLAN zone
horcrux-cli sdn zone create \
  --id zone1 \
  --type vxlan \
  --nodes node1,node2,node3

# Create VNet
horcrux-cli sdn vnet create \
  --id vnet100 \
  --zone zone1 \
  --tag 100 \
  --cidr 10.0.1.0/24
```

### Firewall Rules

```bash
# Allow management access
iptables -A INPUT -p tcp --dport 8006 -j ACCEPT

# Allow cluster communication
iptables -A INPUT -p udp --dport 5404:5405 -j ACCEPT  # Corosync

# Allow VNC consoles
iptables -A INPUT -p tcp --dport 5900:5999 -j ACCEPT
```

---

## Security Hardening

### 1. Enable Two-Factor Authentication

```bash
horcrux-cli auth 2fa enable admin
# Scan QR code with authenticator app
horcrux-cli auth 2fa verify admin <code>
```

### 2. Configure TLS

```bash
# Generate self-signed certificate
openssl req -x509 -nodes -days 365 -newkey rsa:4096 \
  -keyout /etc/horcrux/ssl/key.pem \
  -out /etc/horcrux/ssl/cert.pem

# Or use Let's Encrypt
certbot certonly --standalone -d horcrux.example.com
```

### 3. Firewall Hardening

```bash
# Enable distributed firewall
horcrux-cli firewall enable

# Create security group
horcrux-cli firewall security-group create web-server \
  --rule "tcp/80/in" \
  --rule "tcp/443/in"
```

### 4. SELinux/AppArmor

```bash
# For SELinux systems
semodule -i horcrux.pp

# For AppArmor systems
aa-enforce /etc/apparmor.d/horcrux
```

---

## Clustering

### Create Cluster

On first node:
```bash
horcrux-cli cluster create production
```

### Join Cluster

On additional nodes:
```bash
horcrux-cli cluster join \
  --cluster production \
  --node-ip 192.168.1.101 \
  --join-ip 192.168.1.100
```

### Mixed-Architecture Clustering

```bash
# x86_64 node
horcrux-cli cluster join --arch x86_64 ...

# ARM64 node
horcrux-cli cluster join --arch aarch64 ...

# RISC-V node
horcrux-cli cluster join --arch riscv64 ...
```

### HA Configuration

```bash
# Enable HA for VM
horcrux-cli ha enable vm-100 \
  --priority 100 \
  --affinity "node:node1,node2"

# Create affinity rule
horcrux-cli cluster affinity create \
  --type node \
  --policy required \
  --nodes node1,node2 \
  --resources vm-*
```

---

## Backup Configuration

### Local Backups

```bash
# Create backup job
horcrux-cli backup job create \
  --name daily-backup \
  --schedule "0 2 * * *" \
  --vms "vm-*" \
  --mode snapshot \
  --compression zstd \
  --keep-daily 7 \
  --keep-weekly 4
```

### External Backup Providers

```bash
# Configure S3 backup
horcrux-cli backup provider add s3-backup \
  --type s3 \
  --endpoint https://s3.amazonaws.com \
  --bucket horcrux-backups \
  --access-key <key> \
  --secret-key <secret>

# Backup to S3
horcrux-cli backup create vm-100 \
  --provider s3-backup \
  --compression zstd
```

---

## Monitoring Setup

### Prometheus Integration

`/etc/prometheus/prometheus.yml`:
```yaml
scrape_configs:
  - job_name: 'horcrux'
    static_configs:
      - targets: ['localhost:8006']
    metrics_path: '/api/monitoring/metrics'
```

### OpenTelemetry

```bash
# Configure OTLP export
horcrux-cli observability config \
  --enable \
  --endpoint http://otlp-collector:4318 \
  --interval 60
```

### Grafana Dashboards

```bash
# Import Horcrux dashboard
grafana-cli plugins install horcrux-dashboard
```

### Alert Configuration

```bash
# Create CPU alert
horcrux-cli alerts rule create \
  --name high-cpu \
  --metric cpu_usage \
  --threshold 80 \
  --severity warning \
  --notification email:admin@example.com
```

---

## Troubleshooting

### Check Service Status

```bash
# OpenRC
rc-service horcrux status

# systemd
systemctl status horcrux

# View logs
journalctl -u horcrux -f
```

### Verify Cluster

```bash
horcrux-cli cluster status
```

### Test Connectivity

```bash
# API health check
curl http://localhost:8006/api/health

# Cluster communication
corosync-cfgtool -s
```

---

## Production Checklist

- [ ] System meets minimum requirements
- [ ] Horcrux installed and running
- [ ] TLS certificates configured
- [ ] Two-Factor Authentication enabled
- [ ] Firewall rules applied
- [ ] Cluster configured (if multi-node)
- [ ] Storage pools configured
- [ ] Backup jobs scheduled
- [ ] Monitoring integrated
- [ ] Alert rules configured
- [ ] Documentation reviewed

---

## Next Steps

1. Create your first VM: See [USER_MANUAL.md](USER_MANUAL.md)
2. Configure advanced SDN: See [SDN Guide](docs/SDN.md)
3. Set up HA clustering: See [HA Guide](docs/HA.md)
4. Integrate with CI/CD: See [API Documentation](API_DOCUMENTATION.md)

---

## Support

- Documentation: https://docs.horcrux.io
- Community: https://community.horcrux.io
- Issues: https://github.com/yourusername/horcrux/issues
