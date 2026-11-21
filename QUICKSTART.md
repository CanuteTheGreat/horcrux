# Horcrux Quick Start Guide

Get Horcrux up and running in 5 minutes!

## Prerequisites

- **Linux system** (Gentoo recommended, but any distro works)
- **Rust 1.82+** installed
- **5 minutes** of your time

## Step 1: Install Rust (if needed)

```bash
# Install Rust using rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Verify installation
rustc --version
```

## Step 2: Clone and Build

```bash
# Clone the repository
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux

# Build (takes ~2-3 minutes)
cargo build --release

# Verify build
./target/release/horcrux-api --version
./target/release/horcrux-cli --version
```

## Step 3: Quick Install (Optional)

```bash
# Install to /usr/local
sudo ./install.sh

# Or install with systemd service
sudo ./install.sh --with-systemd
```

## Step 4: Start the API Server

### Option A: Direct Run (Development)
```bash
# Start the API server
./target/release/horcrux-api

# Server starts on https://localhost
```

### Option B: With systemd (Production)
```bash
# If you used --with-systemd during install
sudo systemctl start horcrux
sudo systemctl enable horcrux

# Check status
sudo systemctl status horcrux
```

### Option C: With OpenRC (Gentoo)
```bash
# If you used --with-openrc during install
sudo rc-service horcrux start
sudo rc-update add horcrux default

# Check status
sudo rc-service horcrux status
```

## Step 5: Create Your First VM

### Using the CLI

```bash
# Set the API server (if not localhost)
export HORCRUX_SERVER=https://localhost

# Create a VM
./target/release/horcrux-cli vm create \
  --name "my-first-vm" \
  --cpus 2 \
  --memory 2048 \
  --disk-size 20 \
  --hypervisor qemu

# List VMs
./target/release/horcrux-cli vm list

# Start the VM
./target/release/horcrux-cli vm start my-first-vm

# Check status
./target/release/horcrux-cli vm status my-first-vm

# Stop the VM
./target/release/horcrux-cli vm stop my-first-vm
```

### Using the API Directly

```bash
# Create a VM
curl -X POST https://localhost/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-first-vm",
    "hypervisor": "Qemu",
    "architecture": "X86_64",
    "cpus": 2,
    "memory": 2048,
    "disk_size": 20
  }'

# List VMs
curl https://localhost/api/vms

# Start VM
curl -X POST https://localhost/api/vms/my-first-vm/start

# Get VM status
curl https://localhost/api/vms/my-first-vm
```

## Step 6: Access the Web UI (Optional)

### Build the Web UI

```bash
# Install trunk (one-time)
cargo install trunk

# Add WASM target (one-time)
rustup target add wasm32-unknown-unknown

# Build and serve the UI
cd horcrux-api/horcrux-ui
trunk serve --release
```

Access at: **http://localhost:8080**

## Common Tasks

### Storage Management

```bash
# List storage pools
horcrux-cli storage list

# Create a directory storage pool
horcrux-cli storage create \
  --name "local-storage" \
  --type directory \
  --path /var/lib/horcrux/storage
```

### Cluster Operations

```bash
# Get cluster status
horcrux-cli cluster status

# Add a node
horcrux-cli cluster join \
  --node-id node2 \
  --hostname node2.local \
  --ip 192.168.1.102
```

### Backups

```bash
# Create a backup
horcrux-cli backup create \
  --vm my-first-vm \
  --type full \
  --compression zstd

# List backups
horcrux-cli backup list

# Restore a backup
horcrux-cli backup restore \
  --backup-id <backup-id> \
  --target-vm restored-vm
```

### User Management

```bash
# Create a user
horcrux-cli user create \
  --username alice \
  --email alice@example.com \
  --role VmUser

# List users
horcrux-cli user list

# Change password
horcrux-cli user password \
  --username alice
```

## Configuration

### Basic Configuration

Edit `/etc/horcrux/config.toml` (or `config.toml` in the project root):

```toml
[server]
host = "0.0.0.0"
port = 443
workers = 4

[database]
path = "/var/lib/horcrux/horcrux.db"

[storage]
default_pool = "local"

[auth]
session_timeout = 7200  # 2 hours
```

### Environment Variables

```bash
# API server configuration
export HORCRUX_SERVER=https://localhost
export HORCRUX_LOG_LEVEL=info
export RUST_LOG=horcrux_api=debug

# Database location
export DATABASE_URL=sqlite:///var/lib/horcrux/horcrux.db
```

## Verification

### Check API Health

```bash
curl https://localhost/api/health
```

Expected response:
```json
{"status": "healthy"}
```

### Check Available Hypervisors

```bash
# Check QEMU
which qemu-system-x86_64

# Check LXC
which lxc-info

# Check Docker
docker --version

# Check Podman
podman --version
```

### View Logs

```bash
# If running with systemd
sudo journalctl -u horcrux -f

# If running with OpenRC
tail -f /var/log/horcrux/horcrux.log

# If running directly
# Logs print to stdout
```

## Troubleshooting

### API Server Won't Start

**Problem:** Port 443 (or 80) already in use
```bash
# Check what's using the port
sudo lsof -i :443

# Change port in config.toml
bind_address = "0.0.0.0:8007"
```

**Problem:** Permission denied
```bash
# Check database directory permissions
ls -la /var/lib/horcrux/

# Fix permissions
sudo chown -R $USER:$USER /var/lib/horcrux/
```

### VM Won't Start

**Problem:** QEMU not found
```bash
# Install QEMU
sudo apt install qemu-system-x86  # Debian/Ubuntu
sudo emerge qemu                   # Gentoo
```

**Problem:** Insufficient permissions
```bash
# Add user to kvm group
sudo usermod -aG kvm $USER

# Re-login or
newgrp kvm
```

### CLI Connection Issues

**Problem:** Connection refused
```bash
# Check if API server is running
curl https://localhost/api/health

# Check server address
export HORCRUX_SERVER=https://localhost

# Or specify in command
horcrux-cli --server https://localhost vm list
```

## Next Steps

Now that you have Horcrux running:

1. **Read the full documentation**
   - [API Documentation](docs/API.md)
   - [RBAC Guide](docs/RBAC.md)
   - [Deployment Guide](DEPLOYMENT.md)

2. **Explore features**
   - Set up clustering
   - Configure storage backends (ZFS, Ceph, LVM)
   - Create backup schedules
   - Set up monitoring and alerts

3. **Join the community**
   - [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
   - [Discussions](https://github.com/CanuteTheGreat/horcrux/discussions)

4. **Contribute**
   - Read [CONTRIBUTING.md](CONTRIBUTING.md)
   - Submit bug reports or feature requests
   - Improve documentation

## Quick Reference Card

```bash
# VM Management
horcrux-cli vm create --name <name> --cpus <n> --memory <mb>
horcrux-cli vm list
horcrux-cli vm start <name>
horcrux-cli vm stop <name>
horcrux-cli vm delete <name>

# Storage
horcrux-cli storage list
horcrux-cli storage create --name <name> --type <type> --path <path>

# Backups
horcrux-cli backup create --vm <name>
horcrux-cli backup list
horcrux-cli backup restore --backup-id <id> --target-vm <name>

# Cluster
horcrux-cli cluster status
horcrux-cli cluster join --node-id <id> --ip <address>

# Monitoring
horcrux-cli monitor node
horcrux-cli monitor vm <name>

# Users
horcrux-cli user create --username <name> --role <role>
horcrux-cli user list
```

## Getting Help

- **Documentation**: See `docs/` directory
- **Issues**: https://github.com/CanuteTheGreat/horcrux/issues
- **Discussions**: https://github.com/CanuteTheGreat/horcrux/discussions
- **API Reference**: https://localhost/api/docs (when server is running)

---

**Congratulations!** You now have a working Horcrux installation. 🎉

Start building your virtualization infrastructure with the power of Rust!
