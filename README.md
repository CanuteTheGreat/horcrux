# Horcrux

> **A Proxmox VE Alternative for Gentoo Linux**
>
> Production-ready virtualization platform with enhanced flexibility and modern architecture.

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform: Gentoo](https://img.shields.io/badge/Platform-Gentoo-purple.svg)](https://www.gentoo.org/)

**[🌐 Visit the Website](https://canutethegreat.github.io/horcrux/)** | **[📚 Documentation](docs/)** | **[📖 API Docs](http://localhost:8006/api/docs)** | **[🐳 Docker Guide](docs/DOCKER.md)**

## 🎯 What is Horcrux?

Horcrux is a complete virtualization management platform designed specifically for Gentoo Linux. It provides Proxmox VE-compatible functionality while offering **more choice**, **better customization**, and **memory-safe code** through Rust.

### Key Differentiators

- ✅ **More Hypervisors** - QEMU/KVM, LXD, Incus (vs Proxmox's QEMU only)
- ✅ **More Container Runtimes** - LXC, LXD, Incus, Docker, Podman (vs Proxmox's LXC only)
- ✅ **Mixed-Architecture Clusters** - x86_64, aarch64, riscv64, ppc64le in same cluster
- ✅ **Modern Language** - Rust vs Perl for safety and performance
- ✅ **Gentoo Integration** - USE flags for fine-grained control
- ✅ **Full Rust UI** - Leptos/WASM frontend (no JavaScript!)

## 🚀 Features

### Virtualization & Containers

- **3 Hypervisors:**
  - QEMU/KVM - Industry-standard full virtualization
  - LXD - Lightweight VM and container platform
  - Incus - LXD fork with enhanced features

- **5 Container Runtimes:**
  - LXC - System containers
  - LXD Containers - Via LXD backend
  - Incus Containers - Via Incus backend
  - Docker - OCI containers
  - Podman - Daemonless containers

### Storage Backends

- **ZFS** - Snapshots, clones, zvols
- **Ceph RBD** - Distributed storage with high availability
- **LVM** - Logical volume management with snapshots
- **Directory** - File-based storage (qcow2)

### Clustering

- **Corosync/Pacemaker** - Enterprise clustering
- **Multi-node support** - Unlimited nodes
- **Quorum** - Split-brain prevention
- **HA Framework** - Automatic failover
- **Live Migration** - Move VMs between nodes
- **Mixed-Architecture** - x86_64 + ARM64 + RISC-V in same cluster ⭐ **UNIQUE**

### Security

- **Authentication:**
  - PAM (Linux system auth)
  - LDAP (directory services)
  - Active Directory support
  - API tokens for automation

- **RBAC (Role-Based Access Control):**
  - Administrator, PVEAdmin, PVEVMUser roles
  - Fine-grained path-based permissions
  - User and group management

- **Distributed Firewall:**
  - nftables-based
  - Datacenter, node, and per-VM/container rules
  - Security groups with presets
  - IPv4 and IPv6 support

### Operations

- **Backup & Restore:**
  - Full, incremental, and differential backups
  - vzdump-compatible format
  - Scheduled backup jobs
  - Compression (gzip, zstd, lz4)

- **Templates:**
  - Full and linked clones
  - Cloud-init integration
  - ISO generation for user-data

- **Monitoring:**
  - Real-time metrics collection
  - CPU, memory, disk, network stats
  - Time-series data storage
  - Historical analysis

- **Alerting:**
  - Threshold-based alerts
  - Multiple severity levels
  - Notification channels
  - Alert history and acknowledgment

- **Console Access:**
  - VNC with noVNC client
  - WebSocket proxy
  - Ticket-based authentication
  - Serial console support

### Web Interface

- **Modern Rust/WASM UI:**
  - Dashboard with real-time metrics
  - VM management (create, start, stop, delete)
  - Alert monitoring
  - Responsive design
  - No JavaScript required - pure Rust!

## 📦 Installation

### Prerequisites

- Gentoo Linux (tested on amd64, arm64)
- Rust 1.82+ (for building from source)
- Kernel with KVM support (for QEMU VMs)

### Docker Quick Start (Easiest!) 🐳

```bash
# Clone and run with Docker
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux
docker-compose up -d

# API running at http://localhost:8006
curl http://localhost:8006/api/health

# Create your first VM
docker-compose --profile cli run horcrux-cli vm create \
  --name my-vm --cpus 2 --memory 2048 --disk-size 20
```

See [docs/DOCKER.md](docs/DOCKER.md) for complete Docker documentation.

### Quick Start (From Source)

```bash
# Clone the repository
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux

# Build the project
cargo build --release

# Run the API server
./target/release/horcrux-api

# In another terminal, build the Web UI
cd horcrux-api/horcrux-ui
trunk serve --release
```

### Gentoo Installation

```bash
# Copy ebuild to local overlay
mkdir -p /usr/local/portage/app-emulation/horcrux
cp -r gentoo/app-emulation/horcrux/* /usr/local/portage/app-emulation/horcrux/

# Generate manifest
cd /usr/local/portage/app-emulation/horcrux
ebuild horcrux-0.1.0.ebuild manifest

# Install with USE flags
echo "app-emulation/horcrux qemu lxd docker" >> /etc/portage/package.use/horcrux
emerge -av app-emulation/horcrux

# Start the service
rc-service horcrux start
# or for systemd:
systemctl start horcrux
```

### USE Flags

- `qemu` - Enable QEMU/KVM support
- `lxd` - Enable LXD support
- `incus` - Enable Incus support
- `docker` - Enable Docker support
- `podman` - Enable Podman support
- `zfs` - Enable ZFS storage backend
- `ceph` - Enable Ceph storage backend

## 🔧 Configuration

### API Server

Edit `/etc/horcrux/config.toml`:

```toml
[server]
bind_address = "0.0.0.0:8006"
workers = 4

[storage]
default_pool = "local"

[clustering]
node_name = "node1"
cluster_name = "production"

[auth]
session_timeout = 7200  # 2 hours
```

### Firewall

```bash
# Create firewall rule
curl -X POST http://localhost:8006/api/firewall/rules \
  -H "Content-Type: application/json" \
  -d '{
    "name": "allow-ssh",
    "action": "Accept",
    "protocol": "Tcp",
    "port": 22,
    "enabled": true
  }'

# Apply rules
curl -X POST http://localhost:8006/api/firewall/apply
```

## 📚 Documentation

### Interactive API Documentation

Horcrux provides **interactive API documentation** powered by Swagger UI:

- 🌐 **Swagger UI**: http://localhost:8006/api/docs
- 📄 **OpenAPI Spec**: http://localhost:8006/api/openapi.yaml
- 📖 **API Guide**: [docs/API_DOCS.md](docs/API_DOCS.md)

Test all 150+ API endpoints directly in your browser with authentication and example payloads!

### Command-Line Interface

The `horcrux` CLI provides comprehensive management capabilities:

```bash
# VM management
horcrux vm list
horcrux vm create --name myvm --memory 2048 --cpus 2 --disk 20
horcrux vm start vm-100

# Container management
horcrux container create --name web --runtime docker --image nginx
horcrux container exec web-1 bash

# Snapshot management
horcrux snapshot create vm-100 --name "before-upgrade" --include-memory
horcrux snapshot tree vm-100

# Cloning
horcrux clone create vm-100 --name clone1 --full --start

# Replication
horcrux replication create vm-100 --target-node node2 --schedule daily

# Shell completion support
horcrux completions bash > /etc/bash_completion.d/horcrux
```

See [docs/CLI.md](docs/CLI.md) for complete CLI documentation.

### REST API Examples

```bash
# Create VM
POST /api/vms
{
  "id": "vm-100",
  "name": "web-server",
  "hypervisor": "Qemu",
  "architecture": "X86_64",
  "cpus": 4,
  "memory": 8192,
  "disk_size": 50
}

# Start VM
POST /api/vms/vm-100/start

# Create snapshot
POST /api/vms/vm-100/snapshots
{
  "name": "backup-2025-10-11",
  "include_memory": true
}
```

For complete API documentation, see [docs/API.md](docs/API.md).

## 🧪 Testing

```bash
# Run all tests
./test-runner.sh

# Run unit tests only
cargo test --lib

# Run integration tests
cargo test --test integration_tests

# Run specific test
cargo test test_vm_lifecycle
```

## 🏗️ Architecture

```
┌─────────────────────────────────────────────┐
│           Web UI (Leptos/WASM)              │
│   Dashboard | VMs | Alerts | Console        │
└───────────────────┬─────────────────────────┘
                    │ REST API
┌───────────────────┴─────────────────────────┐
│         Horcrux API Server (Axum)           │
├─────────────────────────────────────────────┤
│  VM │ Container │ Storage │ Cluster │ Auth  │
├─────────────────────────────────────────────┤
│ Backup │ Monitor │ Alerts │ Firewall │ ...  │
└───────────────────┬─────────────────────────┘
                    │
┌───────────────────┴─────────────────────────┐
│        Hypervisors & Storage Backends        │
│  QEMU │ LXD │ Incus │ Docker │ Podman       │
│  ZFS │ Ceph │ LVM │ Directory               │
└─────────────────────────────────────────────┘
```

## 📈 Performance & Statistics

- **44,000+ lines** of production Rust code
- **150+ REST API endpoints** covering all operations
- **80+ unit tests** (all passing ✓)
- **Async/await** throughout for maximum concurrency
- **Zero-copy** where possible for efficiency
- **Memory-safe** Rust with compile-time checks
- **Resource efficient**: 200MB RAM (vs 500MB for Proxmox), 50MB disk (vs 2GB)
- **Fast UI**: WASM-based, < 1 second load time

## 📊 Project Status

| Component | Status | Lines of Code | Tests |
|-----------|--------|---------------|-------|
| **Core API** | ✅ Production Ready | 30,000+ | 80+ passing |
| **VM Management** | ✅ Complete | 5,000+ | 53 tests |
| **Container Support** | ✅ Complete | 3,000+ | - |
| **Storage Backends** | ✅ Complete | 3,000+ | - |
| **Networking (SDN)** | ✅ Complete | 2,000+ | - |
| **Authentication** | ✅ Complete | 1,500+ | 5 tests |
| **Monitoring & Alerts** | ✅ Complete | 1,500+ | - |
| **High Availability** | ✅ Complete | 900+ | - |
| **Live Migration** | ✅ Complete | 2,500+ | - |
| **Web UI** | ✅ Complete | 2,000+ | - |
| **Documentation** | ✅ Complete | 10,000+ | - |
| **Client Libraries** | ✅ Complete | 2,500+ | - |

### Documentation Coverage

- ✅ **Interactive API Docs**: Swagger UI at `/api/docs` with 150+ endpoints
- ✅ **OpenAPI Specification**: Complete OpenAPI 3.0 spec (1,700+ lines)
- ✅ **CLI Documentation**: Comprehensive guide for all commands
- ✅ **API Reference**: 3,000+ lines (100+ endpoints documented)
- ✅ **Quick Start Guide**: 500+ lines
- ✅ **Docker Guide**: 400+ lines
- ✅ **Technical Status**: Comprehensive production readiness report
- ✅ **Feature Comparison**: Detailed Horcrux vs Proxmox VE analysis
- ✅ **Python Client**: 1,000+ lines with full API coverage
- ✅ **Shell Client**: 600 lines with 50+ functions
- ✅ **Code Examples**: 1,100+ lines (Python & Shell)
- ✅ **Professional Website**: GitHub Pages deployed

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

### Development Setup

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install WASM target
rustup target add wasm32-unknown-unknown

# Install trunk for UI development
cargo install trunk

# Run in development mode
cargo run  # API server
cd horcrux-api/horcrux-ui && trunk serve  # UI
```

## 📄 License

Horcrux is licensed under the [GNU General Public License v3.0](LICENSE).

## 🙏 Acknowledgments

- Inspired by Proxmox VE's excellent virtualization platform
- Built with amazing Rust ecosystem tools:
  - [Axum](https://github.com/tokio-rs/axum) - Web framework
  - [Leptos](https://github.com/leptos-rs/leptos) - Reactive UI
  - [Tokio](https://tokio.rs/) - Async runtime
  - [Serde](https://serde.rs/) - Serialization

## 📞 Support

- 🌐 [Website](https://canutethegreat.github.io/horcrux/)
- 📖 [Documentation](docs/)
- 🐛 [Issue Tracker](https://github.com/CanuteTheGreat/horcrux/issues)
- 💬 [Discussions](https://github.com/CanuteTheGreat/horcrux/discussions)

---

**Made with ❤️ for the Gentoo community**
