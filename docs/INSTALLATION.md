# Horcrux Installation Guide

## Table of Contents

- [System Requirements](#system-requirements)
- [Quick Install (Automated)](#quick-install-automated)
- [Manual Installation](#manual-installation)
- [Gentoo Ebuild Installation](#gentoo-ebuild-installation)
- [Post-Installation Setup](#post-installation-setup)
- [Configuration](#configuration)
- [Troubleshooting](#troubleshooting)

## System Requirements

### Minimum Requirements

- **OS**: Gentoo Linux (recommended) or any modern Linux distribution
- **CPU**: 4 cores (x86_64 with VT-x/AMD-V support for KVM)
- **RAM**: 8 GB
- **Disk**: 50 GB free space
- **Rust**: 1.82.0 or later

### Recommended Requirements

- **OS**: Gentoo Linux
- **CPU**: 8+ cores with VT-x/AMD-V
- **RAM**: 16+ GB
- **Disk**: 500+ GB (SSD recommended)
- **Network**: 1 Gbps

### Software Dependencies

**Required:**
- Rust 1.82.0+ (install from https://rustup.rs)
- SQLite 3
- OpenSSL
- systemd

**Optional (for full features):**
- QEMU/KVM (for virtual machines)
- LXC (for containers)
- Docker or Podman (for container support)
- Open vSwitch (for advanced networking)
- ZFS, Ceph, or LVM (for advanced storage)

## Quick Install (Automated)

The easiest way to install Horcrux on Gentoo:

```bash
# Clone the repository
git clone https://github.com/yourusername/horcrux.git
cd horcrux

# Run the installation script
sudo ./deploy/install.sh
```

This script will:
1. Check system requirements
2. Create horcrux user and group
3. Build Horcrux from source
4. Install binaries and Web UI
5. Configure systemd services
6. Initialize the database
7. Generate shell completions

**Installation takes approximately 5-10 minutes** depending on your system.

## Manual Installation

### Step 1: Install Dependencies

**Gentoo:**
```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Install system dependencies
emerge -av dev-db/sqlite dev-libs/openssl sys-apps/systemd

# Optional: Install virtualization tools
emerge -av app-emulation/qemu app-emulation/lxc
```

**Other Distributions:**
```bash
# Ubuntu/Debian
apt-get install build-essential libsqlite3-dev libssl-dev pkg-config

# Fedora/RHEL
dnf install gcc sqlite-devel openssl-devel pkgconfig
```

### Step 2: Build Horcrux

```bash
# Clone repository
git clone https://github.com/yourusername/horcrux.git
cd horcrux

# Build release binaries
cargo build --release -p horcrux-api
cargo build --release -p horcrux-cli

# Build Web UI (requires trunk)
cargo install trunk
cd horcrux-api/horcrux-ui
trunk build --release
cd ../..
```

### Step 3: Create User and Directories

```bash
# Create horcrux user
sudo groupadd -r horcrux
sudo useradd -r -g horcrux -d /var/lib/horcrux -s /bin/bash horcrux

# Add to required groups
sudo usermod -a -G kvm,libvirt,docker horcrux

# Create directories
sudo mkdir -p /opt/horcrux
sudo mkdir -p /etc/horcrux
sudo mkdir -p /var/lib/horcrux/{vms,snapshots,backups,templates,cloudinit}
sudo mkdir -p /var/log/horcrux

# Set ownership
sudo chown -R horcrux:horcrux /var/lib/horcrux
sudo chown -R horcrux:horcrux /var/log/horcrux
```

### Step 4: Install Binaries

```bash
# Install binaries
sudo cp target/release/horcrux-api /usr/bin/
sudo cp target/release/horcrux /usr/bin/
sudo chmod 755 /usr/bin/horcrux*

# Install Web UI
sudo cp -r horcrux-api/horcrux-ui/dist /opt/horcrux/
```

### Step 5: Install Configuration

```bash
# Copy configuration
sudo cp deploy/config/horcrux.toml /etc/horcrux/

# Generate random JWT secret
JWT_SECRET=$(openssl rand -base64 32)
sudo sed -i "s/CHANGE_ME_TO_A_RANDOM_SECRET_KEY/$JWT_SECRET/" /etc/horcrux/horcrux.toml
```

### Step 6: Initialize Database

```bash
# Create database
sudo -u horcrux sqlite3 /var/lib/horcrux/horcrux.db < deploy/scripts/init-db.sql

# Set permissions
sudo chmod 640 /var/lib/horcrux/horcrux.db
sudo chown horcrux:horcrux /var/lib/horcrux/horcrux.db
```

### Step 7: Install Systemd Services

```bash
# Copy service files
sudo cp deploy/systemd/*.service /etc/systemd/system/

# Reload systemd
sudo systemctl daemon-reload

# Enable services
sudo systemctl enable horcrux-api.service
sudo systemctl enable horcrux-metrics.service

# Start services
sudo systemctl start horcrux-api.service
```

### Step 8: Install Shell Completions

```bash
# Bash
sudo horcrux completions bash > /etc/bash_completion.d/horcrux

# Zsh
sudo mkdir -p /usr/share/zsh/site-functions
sudo horcrux completions zsh > /usr/share/zsh/site-functions/_horcrux

# Fish
sudo mkdir -p /usr/share/fish/completions
sudo horcrux completions fish > /usr/share/fish/completions/horcrux.fish
```

## Gentoo Ebuild Installation

### Add to Local Overlay

```bash
# Create local overlay if it doesn't exist
sudo mkdir -p /var/db/repos/local/app-emulation/horcrux

# Copy ebuild
sudo cp deploy/gentoo/horcrux-0.1.0.ebuild /var/db/repos/local/app-emulation/horcrux/

# Generate manifest
cd /var/db/repos/local/app-emulation/horcrux
sudo ebuild horcrux-0.1.0.ebuild manifest
```

### Install via Portage

```bash
# Install with USE flags
sudo emerge -av app-emulation/horcrux

# Or with specific features
sudo USE="qemu lxc docker cluster" emerge -av app-emulation/horcrux
```

### USE Flags

| USE Flag | Description | Default |
|----------|-------------|---------|
| `qemu` | Enable QEMU/KVM support | Yes |
| `lxc` | Enable LXC container support | Yes |
| `docker` | Enable Docker support | No |
| `podman` | Enable Podman support | No |
| `cluster` | Enable clustering features | No |
| `backup` | Enable backup features | No |
| `gpu` | Enable GPU passthrough | No |

## Post-Installation Setup

### 1. Verify Installation

```bash
# Check service status
sudo systemctl status horcrux-api

# View logs
sudo journalctl -u horcrux-api -f

# Check version
horcrux --version
```

### 2. Access Web UI

Open your browser and navigate to:
```
http://localhost:8006
```

You should see the Horcrux dashboard.

### 3. Create Admin User

```bash
# Register first user (will be admin)
horcrux auth register

# Or via API
curl -X POST http://localhost:8006/api/auth/register \
  -H "Content-Type: application/json" \
  -d '{
    "username": "admin",
    "password": "your_secure_password",
    "email": "admin@example.com"
  }'
```

### 4. Login

```bash
# Login via CLI
horcrux auth login

# Or via API
curl -X POST http://localhost:8006/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{
    "username": "admin",
    "password": "your_secure_password"
  }'
```

### 5. Verify KVM Support

```bash
# Check if KVM is available
ls -la /dev/kvm

# Check if user is in kvm group
groups horcrux | grep kvm

# Test QEMU
horcrux vm list
```

## Configuration

### Main Configuration File

Edit `/etc/horcrux/horcrux.toml`:

```toml
[server]
bind_address = "0.0.0.0"
bind_port = 8006

[database]
path = "/var/lib/horcrux/horcrux.db"

[authentication]
jwt_secret = "your-random-secret-here"
session_timeout = 3600

[vm]
default_memory = 2048
default_cpus = 2
```

See `deploy/config/horcrux.toml` for all available options.

### Environment Variables

You can override config with environment variables:

```bash
export HORCRUX_CONFIG=/etc/horcrux/horcrux.toml
export HORCRUX_DATABASE=/var/lib/horcrux/horcrux.db
export RUST_LOG=debug
```

### Firewall Configuration

```bash
# Allow HTTP/HTTPS
sudo iptables -A INPUT -p tcp --dport 8006 -j ACCEPT

# Allow Prometheus metrics
sudo iptables -A INPUT -p tcp --dport 9090 -j ACCEPT

# Save rules
sudo iptables-save > /etc/iptables/rules.v4
```

## Troubleshooting

### Service Won't Start

```bash
# Check logs
sudo journalctl -u horcrux-api -n 100

# Check configuration
horcrux config check

# Test manually
sudo -u horcrux /usr/bin/horcrux-api
```

### Permission Errors

```bash
# Fix ownership
sudo chown -R horcrux:horcrux /var/lib/horcrux
sudo chown -R horcrux:horcrux /var/log/horcrux

# Check group memberships
groups horcrux

# Add to kvm group
sudo usermod -a -G kvm horcrux
```

### Database Issues

```bash
# Verify database
sudo -u horcrux sqlite3 /var/lib/horcrux/horcrux.db ".schema"

# Rebuild database
sudo -u horcrux sqlite3 /var/lib/horcrux/horcrux.db < deploy/scripts/init-db.sql
```

### Web UI Not Loading

```bash
# Check if dist directory exists
ls -la /opt/horcrux/dist

# Rebuild UI
cd horcrux-api/horcrux-ui
trunk build --release

# Copy to install location
sudo cp -r dist /opt/horcrux/
```

### KVM Not Available

```bash
# Load KVM module
sudo modprobe kvm
sudo modprobe kvm_intel  # or kvm_amd

# Make permanent
echo "kvm" | sudo tee -a /etc/modules-load.d/kvm.conf
echo "kvm_intel" | sudo tee -a /etc/modules-load.d/kvm.conf

# Check CPU support
egrep -c '(vmx|svm)' /proc/cpuinfo
```

### Port Already in Use

```bash
# Check what's using port 8006
sudo netstat -tlnp | grep 8006
sudo lsof -i :8006

# Change port in config
sudo vim /etc/horcrux/horcrux.toml
# [server]
# bind_port = 8007

# Restart service
sudo systemctl restart horcrux-api
```

## Upgrading

### Manual Upgrade

```bash
# Stop services
sudo systemctl stop horcrux-api

# Backup database
sudo -u horcrux cp /var/lib/horcrux/horcrux.db /var/lib/horcrux/horcrux.db.backup

# Pull latest code
cd horcrux
git pull

# Rebuild
cargo build --release -p horcrux-api
cargo build --release -p horcrux-cli

# Install new binaries
sudo cp target/release/horcrux-api /usr/bin/
sudo cp target/release/horcrux /usr/bin/

# Restart services
sudo systemctl start horcrux-api
```

### Ebuild Upgrade

```bash
# Update overlay
cd /var/db/repos/local
git pull

# Upgrade
sudo emerge -uav app-emulation/horcrux
```

## Uninstallation

```bash
# Stop services
sudo systemctl stop horcrux-api horcrux-metrics
sudo systemctl disable horcrux-api horcrux-metrics

# Remove binaries
sudo rm /usr/bin/horcrux*

# Remove services
sudo rm /etc/systemd/system/horcrux*.service
sudo systemctl daemon-reload

# Remove data (WARNING: This deletes all VMs and data!)
sudo rm -rf /var/lib/horcrux
sudo rm -rf /var/log/horcrux
sudo rm -rf /etc/horcrux
sudo rm -rf /opt/horcrux

# Remove user
sudo userdel horcrux
sudo groupdel horcrux
```

## Next Steps

After installation:

1. **Read the documentation**: Check `/usr/share/doc/horcrux/` or `docs/` directory
2. **Create VMs**: See [CLI.md](CLI.md) for commands
3. **Configure networking**: See [NEW_FEATURES.md](NEW_FEATURES.md) for SDN setup
4. **Setup monitoring**: See [REALTIME_FEATURES.md](REALTIME_FEATURES.md) for monitoring
5. **Configure backups**: Setup automated backup policies
6. **Join the community**: Report issues, contribute, get support

## Getting Help

- **Documentation**: `/usr/share/doc/horcrux/` or `docs/` directory
- **Logs**: `journalctl -u horcrux-api -f`
- **CLI Help**: `horcrux --help`
- **API Docs**: http://localhost:8006/api/docs
- **Issues**: https://github.com/yourusername/horcrux/issues

---

**Made with ❤️ for the Gentoo community**
