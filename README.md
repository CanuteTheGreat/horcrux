<p align="center">
  <img src="branding/horcrux-logo-official.png" alt="Horcrux" width="260">
</p>

# Horcrux

> **A Proxmox VE-style virtualization manager for Gentoo Linux, written in Rust.**
>
> **Status: in active development (v0.1.x).** Not yet suitable for production use.
> APIs, config and on-disk formats can still change between releases. Try it on test hardware.

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform: Gentoo](https://img.shields.io/badge/Platform-Gentoo-purple.svg)](https://www.gentoo.org/)

**[Documentation](docs/)** | **[API reference](docs/API_DOCS.md)** | **[Installation](docs/INSTALLATION.md)** | **[Docker guide](docs/DOCKER.md)**

## What is Horcrux?

Horcrux is a web-managed virtualization platform that aims at Proxmox VE-style
functionality (VMs, containers, storage, clustering, firewall, backups, RBAC) on
Gentoo, with a Rust API server, a Rust/WASM web UI, a CLI and a Terraform provider.
It differs from Proxmox in a few deliberate ways:

- **More backends:** QEMU/KVM, LXD and Incus for VMs; LXC, LXD, Incus, Docker and Podman for containers.
- **Kubernetes management:** connect or provision k3s/kubeadm clusters, workloads, Helm.
- **Mixed-architecture clusters:** x86_64, aarch64 and riscv64 nodes in one cluster.
- **Gentoo-native:** ebuilds and USE flags select which features get built.
- **Rust throughout**, including the web UI (Leptos/WASM).

## Current status

What is covered by automated tests in the project's CI today:

- **Build and lint:** formatting, clippy, release builds across feature sets, and a USE-flag feature build matrix.
- **Unit and integration tests** run against a live `horcrux-api` server.
- **Against real services, not mocks:** OpenLDAP, Dex (OIDC), PAM and an ACME test CA for auth realms and certificates; real nftables for firewall and SDN policy enforcement; real CNI plugins; real tap/VLAN/PXE VM networking; nested LXC and Incus containers; a k3s cluster with a local chart repository for Helm; the Docker daemon for container tests.
- **Web UI:** built to WASM and exercised by browser end-to-end tests.
- **Gentoo:** the container image builds through Portage, and the ebuild installs across a USE-flag matrix.
- **Security audit** (`cargo audit`) runs on every change.

Known gaps and caveats:

- **riscv64 / aarch64** release binaries are cross-compiled, but there is no physical RISC-V hardware in the test setup. Reports from real riscv64 boards are welcome.
- The `Hypervisor` HA fencing agent type is a stub (see `horcrux-api/src/ha/fencing.rs`); the IPMI, iLO/DRAC, SSH, libvirt, watchdog and SNMP-PDU agents are implemented.
- The mobile UI's Storage and Network pages are placeholders ("coming soon").
- Several status/summary documents in the repo root are historical session notes and may overstate completeness; treat this README, `docs/` and the CI results as the source of truth. See also [ROADMAP.md](ROADMAP.md) and [MISSING_FEATURES.md](MISSING_FEATURES.md).
- No third-party security review has been done.

## Features

### Virtualization and containers

- **Hypervisors:** QEMU/KVM, LXD, Incus.
- **Container runtimes:** LXC, LXD, Incus, Docker, Podman.
- **Templates:** full and linked clones, cloud-init.
- **Console access:** VNC (noVNC in the browser via a WebSocket proxy; QEMU listens on 127.0.0.1 with a per-start password), SPICE, serial console, headless VMs.

### Kubernetes

- Connect clusters via kubeconfig or provision k3s/kubeadm clusters.
- Pods, Deployments, StatefulSets, DaemonSets, Jobs/CronJobs, Services, Ingress, NetworkPolicies, ConfigMaps/Secrets, PVCs, StorageClasses.
- Helm install/upgrade/rollback and repository management; metrics, events, logs, exec over WebSocket.

See [docs/KUBERNETES.md](docs/KUBERNETES.md).

### Storage

ZFS, Ceph RBD, LVM, directory (qcow2) and iSCSI backends.

### Clustering and HA

- Corosync/Pacemaker clustering with quorum checks.
- HA framework with affinity rules and fencing agents.
- Live migration of running QEMU VMs (disk mirror + QEMU migration, no shared storage required), or offline migration with a verified copy; cancel rolls back. See [docs/MIGRATION.md](docs/MIGRATION.md).

### Security

- **Realms** (`user@realm`): local, PAM, LDAP (plain/StartTLS/LDAPS), Active Directory, OpenID Connect (PKCE, state, nonce), and API keys.
- **Second factors:** TOTP, WebAuthn security keys/passkeys, recovery codes; enforceable per user, realm or globally.
- **RBAC:** Proxmox-style privileges checked on every API route, built-in and custom roles, path ACLs with propagation.
- **HTTPS by default** on port 8006 using a node-local CA (or your own certificate, or ACME/Let's Encrypt with renewal). Plain HTTP is redirected.
- **Firewall:** nftables-based, datacenter/node/per-guest rules, security groups, IPv4 and IPv6.

See [docs/ACCESS_CONTROL.md](docs/ACCESS_CONTROL.md).

### Operations

- **Backup and restore:** full/incremental/differential, vzdump-compatible format, scheduled jobs, gzip/zstd/lz4.
- **Monitoring and alerting:** node and container metrics (/proc, cgroups v1/v2), live dashboard over WebSocket, thresholds, notification channels, alert history.
- **Web UI:** Rust/WASM dashboard, VM management, alerts, consoles.
- **CLI** (`horcrux`), **mobile UI** (`horcrux-mobile`) and a **Terraform provider** (`terraform-provider-horcrux`: `horcrux_vm`, `horcrux_container`, `horcrux_storage_pool`, `horcrux_firewall_rule`).

## Installation

### Prerequisites

- Gentoo Linux (recommended); other Linux distributions may work but are not the target.
- CPU with VT-x/AMD-V for KVM; 8+ GB RAM and 50+ GB disk are a sensible minimum for guests.
- Rust 1.82 or later to build from source.

### From source (installer script)

```bash
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux
sudo ./deploy/install.sh
```

The script checks requirements, creates the `horcrux` user and groups, builds from
source, installs binaries and the web UI, configures services and initializes the
database. Then open `https://localhost:8006`.

On first start there is no default password: the `admin` password comes from
`ADMIN_PASSWORD`, or is generated into `/var/lib/horcrux/initial-admin-password`
(deleted once you change it). Self-registration is off. Scripts must use
`https://` and trust the node CA from `/api/tls/ca.pem` (or the CLI's `--ca-cert`).
Details: [docs/ACCESS_CONTROL.md](docs/ACCESS_CONTROL.md). The full guide is
[docs/INSTALLATION.md](docs/INSTALLATION.md).

### Docker (development)

The Docker path builds through the project's own Gentoo/Portage overlay (`gentoo/`),
so USE flags still apply. It is a development/CI convenience, not a separate
deployment target.

```bash
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux
docker-compose up -d

curl -k https://localhost:8006/api/tls/ca.pem -o horcrux-ca.pem
curl --cacert horcrux-ca.pem https://localhost:8006/api/health
```

See [docs/DOCKER.md](docs/DOCKER.md).

### Gentoo ebuild

```bash
sudo mkdir -p /var/db/repos/local/app-emulation/horcrux
sudo cp deploy/gentoo/horcrux-0.1.0.ebuild /var/db/repos/local/app-emulation/horcrux/
cd /var/db/repos/local/app-emulation/horcrux
sudo ebuild horcrux-0.1.0.ebuild manifest
sudo USE="qemu lxc docker" emerge -av app-emulation/horcrux
sudo systemctl enable --now horcrux-api
```

A live `horcrux-9999.ebuild` is also in `deploy/gentoo/`.

### USE flags

| Flag | Description | Default |
|------|-------------|---------|
| `qemu` | QEMU/KVM virtualization | on |
| `lxc` | LXC containers | on |
| `docker` | Docker integration | on |
| `podman` | Podman integration | off |
| `cluster` | Clustering and HA (`/ha/*` pages, node list, create/join, failover) | on |
| `backup` | Backup compression | on |
| `gpu` | GPU passthrough | off |

## Configuration

Edit `/etc/horcrux/config.toml` (example in `deploy/config.toml.example`):

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

## Using it

### API

The server publishes an OpenAPI spec and Swagger UI on your own node at
`/api/openapi.yaml` and `/api/docs` once it is running. The spec is also in the
repo at [docs/openapi.yaml](docs/openapi.yaml); guides are
[docs/API_DOCS.md](docs/API_DOCS.md) and [docs/API.md](docs/API.md).

```bash
TOKEN=$(curl -s --cacert horcrux-ca.pem -X POST https://localhost:8006/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "<your admin password>"}' | jq -r .ticket)

curl --cacert horcrux-ca.pem -H "Authorization: Bearer $TOKEN" https://localhost:8006/api/vms
```

### CLI

```bash
horcrux vm list
horcrux vm create --name myvm --memory 2048 --cpus 2 --disk 20
horcrux vm start vm-100
horcrux snapshot create vm-100 --name "before-upgrade" --include-memory
horcrux clone create vm-100 --name clone1 --full --start
horcrux completions bash > /etc/bash_completion.d/horcrux
```

See [docs/CLI.md](docs/CLI.md).

## Testing

```bash
./test-runner.sh        # everything
cargo test --lib        # unit tests
cargo test --test integration_tests
```

Many integration tests need real services (nftables, LXC, Incus, k3s, LDAP...) and
are meant to run in CI; the unit tests run anywhere.

## Architecture

```
┌─────────────────────────────────────────────┐
│           Web UI (Leptos/WASM)              │
└───────────────────┬─────────────────────────┘
                    │ REST API / WebSocket
┌───────────────────┴─────────────────────────┐
│         Horcrux API Server (Axum)           │
│  VM │ Container │ Storage │ Cluster │ Auth  │
│ Backup │ Monitor │ Alerts │ Firewall │ SDN  │
└───────────────────┬─────────────────────────┘
┌───────────────────┴─────────────────────────┐
│        Hypervisors & Storage Backends       │
│  QEMU │ LXD │ Incus │ Docker │ Podman       │
│  ZFS │ Ceph │ LVM │ iSCSI │ Directory       │
└─────────────────────────────────────────────┘
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo run                                   # API server
cd horcrux-api/horcrux-ui && trunk serve    # UI
```

## License

GPL-3.0. See [LICENSE](LICENSE).

## Acknowledgments

Inspired by Proxmox VE. Built with [Axum](https://github.com/tokio-rs/axum),
[Leptos](https://github.com/leptos-rs/leptos), [Tokio](https://tokio.rs/) and [Serde](https://serde.rs/).

## Support

- [Documentation](docs/)
- [Issue tracker](https://github.com/CanuteTheGreat/horcrux/issues)
- [Discussions](https://github.com/CanuteTheGreat/horcrux/discussions)
