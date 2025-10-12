# Horcrux CLI Documentation

## Overview

The `horcrux` command-line interface provides comprehensive management capabilities for the Horcrux virtualization platform. It supports VMs, containers, storage, backups, clustering, and more.

## Installation

Build and install the CLI:

```bash
cd horcrux
cargo build -p horcrux-cli --release
sudo cp target/release/horcrux /usr/local/bin/
```

## Basic Usage

```bash
horcrux [OPTIONS] <COMMAND>
```

### Global Options

- `-s, --server <SERVER>`: API server address (default: `http://localhost:8006`)
- `-o, --output <OUTPUT>`: Output format: `table`, `json`, or `yaml` (default: `table`)
- `-h, --help`: Print help
- `-V, --version`: Print version

### Examples

```bash
# List VMs in table format
horcrux vm list

# List VMs in JSON format
horcrux -o json vm list

# Connect to remote server
horcrux -s http://192.168.1.100:8006 vm list
```

## Authentication

Before using most commands, you need to authenticate:

```bash
# Register a new user
horcrux auth register

# Login
horcrux auth login

# Check auth status
horcrux auth status

# Logout
horcrux auth logout
```

Authentication tokens are stored in `~/.config/horcrux/config.toml` and used automatically for subsequent commands.

## Command Reference

### Virtual Machines (`vm`)

Manage virtual machines:

```bash
# List all VMs
horcrux vm list

# Show VM details
horcrux vm show <vm-id>

# Create a new VM
horcrux vm create --name myvm --memory 2048 --cpus 2 --disk 20

# Start a VM
horcrux vm start <vm-id>

# Stop a VM
horcrux vm stop <vm-id>

# Restart a VM
horcrux vm restart <vm-id>

# Delete a VM (with confirmation)
horcrux vm delete <vm-id>

# Clone from template
horcrux vm clone <template-id> --name new-vm
```

### Containers (`container`)

Manage containers (Docker, Podman, LXC, LXD, Incus):

```bash
# List all containers
horcrux container list

# Show container details
horcrux container show <container-id>

# Create a container
horcrux container create \
  --name mycontainer \
  --runtime docker \
  --image ubuntu:22.04 \
  --memory 1024 \
  --cpus 2

# Start a container
horcrux container start <container-id>

# Stop a container
horcrux container stop <container-id>

# Delete a container
horcrux container delete <container-id>

# Execute command in container
horcrux container exec <container-id> ls -la
horcrux container exec <container-id> bash
```

### Snapshots (`snapshot`)

Manage VM snapshots:

```bash
# List snapshots for a VM
horcrux snapshot list <vm-id>

# Show snapshot details
horcrux snapshot show <vm-id> <snapshot-id>

# Create a snapshot
horcrux snapshot create <vm-id> \
  --name "before-upgrade" \
  --description "Snapshot before system upgrade"

# Create snapshot with memory (for running VMs)
horcrux snapshot create <vm-id> \
  --name "running-state" \
  --include-memory

# Restore a snapshot
horcrux snapshot restore <vm-id> <snapshot-id>

# Delete a snapshot
horcrux snapshot delete <vm-id> <snapshot-id>

# Show snapshot tree
horcrux snapshot tree <vm-id>
```

### Cloning (`clone`)

VM cloning operations:

```bash
# Clone a VM
horcrux clone create <source-vm-id> \
  --name cloned-vm \
  --full \
  --start

# List clone jobs
horcrux clone list

# Check clone job status
horcrux clone status <job-id>

# Cancel a clone job
horcrux clone cancel <job-id>
```

**Clone Types:**
- **Linked clone** (default): Fast, shares disk with source VM
- **Full clone** (`--full`): Slower, independent copy with its own disk

### Replication (`replication`)

ZFS replication for disaster recovery:

```bash
# List replication jobs
horcrux replication list

# Show replication job details
horcrux replication show <job-id>

# Create replication job
horcrux replication create <vm-id> \
  --target-node node2 \
  --schedule daily

# Execute replication now
horcrux replication execute <job-id>

# Show replication status
horcrux replication status <job-id>

# Delete replication job
horcrux replication delete <job-id>
```

**Schedules:**
- `hourly`: Replicate every hour
- `daily`: Replicate once per day
- `weekly`: Replicate once per week
- `manual`: Only replicate when manually triggered

### Storage (`storage`)

Manage storage pools:

```bash
# List storage pools
horcrux storage list

# Show storage details
horcrux storage show <pool-id>

# Add a storage pool
horcrux storage add \
  --name mypool \
  --type zfs \
  --path /dev/sdb

# Remove a storage pool
horcrux storage remove <pool-id>

# Create a volume
horcrux storage create-volume <pool-id> <name> <size-gb>
```

**Storage Types:** `zfs`, `ceph`, `lvm`, `directory`, `nfs`, `cifs`, `glusterfs`, `btrfs`, `s3`

### Backups (`backup`)

Backup and restore operations:

```bash
# List backups
horcrux backup list

# Show backup details
horcrux backup show <backup-id>

# Create a backup
horcrux backup create <vm-id> \
  --mode snapshot \
  --compression zstd

# Restore a backup
horcrux backup restore <backup-id>

# Restore to different VM
horcrux backup restore <backup-id> --target <vm-id>

# Delete a backup
horcrux backup delete <backup-id>

# Schedule a backup job
horcrux backup schedule \
  --name "nightly-backup" \
  --schedule "0 2 * * *" \
  --vms "vm-100,vm-101,vm-102"
```

**Backup Modes:**
- `snapshot`: Online backup using snapshots (default)
- `suspend`: Suspend VM during backup
- `stop`: Stop VM during backup

**Compression:** `none`, `lzo`, `gzip`, `zstd` (recommended)

### Clustering (`cluster`)

Multi-node cluster management:

```bash
# List cluster nodes
horcrux cluster list

# Show cluster status
horcrux cluster status

# Show cluster architecture
horcrux cluster architecture

# Add a node
horcrux cluster add <node-name> <node-address>

# Remove a node
horcrux cluster remove <node-name>
```

### High Availability (`ha`)

HA resource management:

```bash
# List HA resources
horcrux ha list

# Show HA status
horcrux ha status

# Add VM to HA
horcrux ha add <vm-id> --group default --priority 100

# Remove VM from HA
horcrux ha remove <vm-id>

# Create HA group
horcrux ha create-group <name> --nodes "node1,node2,node3"
```

### Migration (`migrate`)

Live VM migration:

```bash
# Migrate VM to another node
horcrux migrate <vm-id> <target-node>

# Specify migration type
horcrux migrate <vm-id> <target-node> --migration-type online
```

**Migration Types:**
- `live`: Live migration with minimal downtime
- `online`: Online migration
- `offline`: Offline migration (VM must be stopped)

### Monitoring (`monitor`)

System resource monitoring:

```bash
# Show node metrics
horcrux monitor node

# Show VM metrics
horcrux monitor vm
horcrux monitor vm <vm-id>

# Show storage metrics
horcrux monitor storage
horcrux monitor storage <pool-name>

# Show cluster metrics
horcrux monitor cluster

# Watch metrics in real-time
horcrux monitor watch --interval 2
```

### Users (`user`)

User and permission management:

```bash
# List users
horcrux user list

# Create a user
horcrux user create <username> \
  --password <password> \
  --role operator

# Delete a user
horcrux user delete <username>

# Change password
horcrux user passwd <username>

# List roles
horcrux user roles

# Grant permission
horcrux user grant <username> "VM.Allocate"
```

**Roles:** `admin`, `operator`, `user`

### Audit Logs (`audit`)

Security and audit logging:

```bash
# Query audit logs
horcrux audit query \
  --event-type "vm.create" \
  --user admin \
  --severity info \
  --limit 50

# Show failed login attempts
horcrux audit failed-logins --limit 20

# Show security events
horcrux audit security --limit 20

# Export audit logs
horcrux audit export audit-logs.json
```

**Severity Levels:** `info`, `warning`, `error`, `critical`

## Shell Completion

The CLI supports shell completion for bash, zsh, fish, PowerShell, and Elvish.

### Bash

Add to `~/.bashrc`:

```bash
eval "$(horcrux completions bash)"
```

Or install completion file:

```bash
horcrux completions bash > /etc/bash_completion.d/horcrux
```

### Zsh

Add to `~/.zshrc`:

```zsh
eval "$(horcrux completions zsh)"
```

Or install completion file:

```zsh
horcrux completions zsh > ~/.zsh/completion/_horcrux
```

### Fish

Install completion file:

```fish
horcrux completions fish > ~/.config/fish/completions/horcrux.fish
```

### PowerShell

Add to PowerShell profile:

```powershell
horcrux completions powershell | Out-String | Invoke-Expression
```

## Output Formats

### Table (Default)

Human-readable tabular output:

```bash
horcrux vm list
```

```
┌──────────┬───────────┬─────────┬──────┬───────────┬──────────────┐
│ id       │ name      │ status  │ cpus │ memory_mb │ architecture │
├──────────┼───────────┼─────────┼──────┼───────────┼──────────────┤
│ vm-100   │ webserver │ Running │ 4    │ 8192      │ X86_64       │
│ vm-101   │ database  │ Running │ 8    │ 16384     │ X86_64       │
└──────────┴───────────┴─────────┴──────┴───────────┴──────────────┘
```

### JSON

Machine-readable JSON output:

```bash
horcrux -o json vm list
```

```json
[
  {
    "id": "vm-100",
    "name": "webserver",
    "status": "Running",
    "cpus": 4,
    "memory": 8589934592,
    "architecture": "X86_64",
    "hypervisor": "Qemu"
  }
]
```

### YAML

YAML output for configuration management:

```bash
horcrux -o yaml vm show vm-100
```

```yaml
id: vm-100
name: webserver
status: Running
cpus: 4
memory: 8589934592
architecture: X86_64
hypervisor: Qemu
disk_size: 100
```

## Interactive Features

### Confirmation Prompts

Destructive operations require confirmation:

```bash
horcrux vm delete vm-100
# Prompts: "Are you sure you want to delete VM vm-100?"
```

### Progress Indicators

Long-running operations show progress:

```bash
horcrux snapshot create vm-100 --name backup
# Shows: ⠋ Creating snapshot 'backup'...
```

### Rich Output

- **Success messages**: Green ✓
- **Info messages**: Blue ℹ
- **Error messages**: Red ✗
- **Tables**: Formatted with borders
- **Spinners**: Animated progress for async operations
- **Progress bars**: For tasks with measurable progress

## Configuration

Configuration is stored in `~/.config/horcrux/config.toml`:

```toml
# Authentication token (set automatically on login)
token = "eyJ0eXAiOiJKV1QiLCJhbGc..."

# Default API server
server = "http://localhost:8006"

# Default output format
output = "table"
```

## Examples

### Complete VM Lifecycle

```bash
# Create a VM
horcrux vm create --name testvm --memory 2048 --cpus 2 --disk 20

# Start the VM
horcrux vm start testvm

# Create a snapshot
horcrux snapshot create testvm --name "initial-state"

# Monitor the VM
horcrux monitor vm testvm

# Stop the VM
horcrux vm stop testvm

# Clone the VM
horcrux clone create testvm --name testvm-clone

# Delete the VM
horcrux vm delete testvm
```

### Container Workflow

```bash
# Create and start a container
horcrux container create \
  --name web \
  --runtime docker \
  --image nginx:latest \
  --memory 512 \
  --cpus 1

horcrux container start web

# Execute commands
horcrux container exec web nginx -v
horcrux container exec web cat /etc/nginx/nginx.conf

# Stop and clean up
horcrux container stop web
horcrux container delete web
```

### Backup and Restore

```bash
# Create a backup
horcrux backup create vm-100 --mode snapshot --compression zstd

# List backups to find the backup ID
horcrux backup list

# Restore the backup
horcrux backup restore backup-12345

# Or restore to a different VM
horcrux backup restore backup-12345 --target vm-200
```

### Disaster Recovery with Replication

```bash
# Set up replication to secondary node
horcrux replication create vm-100 \
  --target-node node2 \
  --schedule daily

# Check replication status
horcrux replication status <job-id>

# Force immediate replication
horcrux replication execute <job-id>

# In case of disaster, VMs can be started on node2
```

## Troubleshooting

### Connection Errors

If you get connection errors, verify:

1. API server is running: `systemctl status horcrux-api`
2. Server address is correct: `horcrux -s http://your-server:8006 vm list`
3. Firewall allows port 8006

### Authentication Errors

If authentication fails:

```bash
# Check current auth status
horcrux auth status

# Re-login
horcrux auth login

# Verify token in config
cat ~/.config/horcrux/config.toml
```

### Debug Mode

For detailed error information, use JSON output:

```bash
horcrux -o json vm list 2>&1 | jq
```

## See Also

- **API Documentation**: [docs/API_DOCS.md](API_DOCS.md)
- **Interactive API Docs**: http://localhost:8006/api/docs
- **OpenAPI Spec**: http://localhost:8006/api/openapi.yaml
- **GitHub**: https://github.com/CanuteTheGreat/horcrux

## Support

- **GitHub Issues**: https://github.com/CanuteTheGreat/horcrux/issues
- **Discussions**: https://github.com/CanuteTheGreat/horcrux/discussions

---

**Made with ❤️ for the Gentoo community**
