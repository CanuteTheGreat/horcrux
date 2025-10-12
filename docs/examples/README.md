# Horcrux API Examples

Code examples and client libraries for working with the Horcrux virtualization API.

## Overview

This directory contains complete examples in multiple programming languages, demonstrating how to interact with Horcrux programmatically.

## Available Examples

### 🐍 Python

**Location:** [`python/`](python/)

Complete Python client library with comprehensive examples.

**Features:**
- Full API coverage (100+ methods)
- Type hints and docstrings
- Error handling
- Context manager support
- Helper methods for common operations

**Quick Start:**
```python
from horcrux_client import HorcruxClient

with HorcruxClient("http://localhost:8006", "admin", "admin") as client:
    vms = client.list_vms()
    vm = client.create_vm("my-vm", cpus=2, memory=2048, disk_size=20)
    client.start_vm(vm['id'])
```

**Examples Included:**
- `horcrux_client.py` - Complete client library
- `example_basic.py` - Basic VM operations
- `example_advanced.py` - Advanced features (HA, clustering, backups, monitoring)

[📖 Python Documentation](python/README.md)

---

### 🐚 Shell Scripts

**Location:** [`shell/`](shell/)

Simple bash wrapper using `curl` and `jq` for command-line automation.

**Features:**
- No dependencies except `curl` and `jq`
- Easy to integrate into existing scripts
- Interactive and scriptable usage
- All major API endpoints covered

**Quick Start:**
```bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"
horcrux_list_vms
horcrux_create_vm "my-vm" 2 2048 20
horcrux_logout
```

**Examples Included:**
- `horcrux.sh` - Shell client library
- `example.sh` - Complete workflow example

[📖 Shell Documentation](shell/README.md)

---

## Language Support Summary

| Language | Client Library | Basic Examples | Advanced Examples | Status |
|----------|---------------|----------------|-------------------|--------|
| Python   | ✅            | ✅             | ✅                | Complete |
| Shell    | ✅            | ✅             | ✅                | Complete |
| Go       | ❌            | ❌             | ❌                | Planned |
| Rust     | ❌            | ❌             | ❌                | Planned |
| TypeScript | ❌          | ❌             | ❌                | Planned |

## Common Use Cases

### 1. VM Lifecycle Management

**Python:**
```python
client = HorcruxClient(base_url, username, password)

# Create and configure
vm = client.create_vm("web-server", cpus=4, memory=8192, disk_size=50)
client.start_vm(vm['id'])
client.wait_for_vm_status(vm['id'], "running")

# Snapshot before changes
client.create_snapshot(vm['id'], "before-upgrade")

# Backup
client.create_backup(vm['id'], backup_type="full", compression="zstd")
```

**Shell:**
```bash
source horcrux.sh
horcrux_login "http://localhost:8006" "admin" "admin"

# Create and configure
vm_id=$(horcrux_create_vm "web-server" 4 8192 50 | jq -r '.id')
horcrux_start_vm "$vm_id"

# Snapshot before changes
horcrux_create_snapshot "$vm_id" "before-upgrade"

# Backup
horcrux_create_backup "$vm_id" "full" "zstd"
```

### 2. Automated Backups

**Python:**
```python
client = HorcruxClient(base_url, username, password)

# Backup all running VMs
vms = client.list_vms(status="running")
for vm in vms:
    print(f"Backing up {vm['name']}...")
    backup = client.create_backup(
        vm['id'],
        backup_type="full",
        compression="zstd"
    )
    client.wait_for_backup(backup['id'])
    print(f"  ✓ Backup completed")
```

**Shell:**
```bash
#!/bin/bash
source horcrux.sh
horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

# Backup all running VMs
horcrux_list_vms | jq -r '.[] | select(.status == "running") | .id' | \
while read vm_id; do
    echo "Backing up $vm_id..."
    horcrux_create_backup "$vm_id" "full" "zstd"
done
```

### 3. Cluster Management

**Python:**
```python
client = HorcruxClient(base_url, username, password)

# Get cluster status
nodes = client.list_cluster_nodes()
for node in nodes:
    print(f"{node['name']}: {node['status']} - {node['vms_running']} VMs")

# Find optimal node for new VM
node = client.find_best_node_for_vm(memory=8192, cpus=4, architecture="X86_64")
print(f"Best node: {node['recommended_node']}")
```

**Shell:**
```bash
source horcrux.sh
horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

# Get cluster status
echo "=== Cluster Nodes ==="
horcrux_list_cluster_nodes | jq -r '.[] | "\(.name): \(.status) - \(.vms_running) VMs"'

# Get architecture summary
echo "=== Architecture ==="
horcrux_get_cluster_architecture | jq '.'
```

### 4. Monitoring and Alerting

**Python:**
```python
client = HorcruxClient(base_url, username, password)

# Get node statistics
stats = client.get_node_stats()
print(f"CPU: {stats['cpu_usage']}%")
print(f"Memory: {stats['memory_used']/stats['memory_total']*100:.1f}%")

# Create alert rule
client.create_alert_rule(
    name="High CPU",
    metric="cpu_usage",
    threshold=80.0,
    condition="greater_than",
    severity="warning"
)

# Check active alerts
alerts = client.list_active_alerts()
for alert in alerts:
    print(f"⚠️  {alert['rule_name']}: {alert['severity']}")
```

**Shell:**
```bash
source horcrux.sh
horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

# Get node statistics
echo "=== Node Stats ==="
horcrux_get_node_stats | jq '{cpu: .cpu_usage, memory: .memory_usage}'

# Check active alerts
echo "=== Active Alerts ==="
horcrux_list_active_alerts | jq -r '.[] | "⚠️  \(.rule_name): \(.severity)"'
```

### 5. High Availability Setup

**Python:**
```python
client = HorcruxClient(base_url, username, password)

# Add VMs to HA management
critical_vms = ["database", "load-balancer", "auth-server"]
for vm_name in critical_vms:
    vm = client.list_vms()[0]  # Find VM by name
    client.add_ha_resource(
        vm_id=vm['id'],
        priority=255,  # Highest priority
        group="critical-services"
    )
    print(f"Added {vm_name} to HA")

# Check HA status
status = client.get_ha_status()
print(f"HA enabled: {status['enabled']}")
print(f"Resources: {status['total_resources']}")
```

**Shell:**
```bash
source horcrux.sh
horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

# Add VMs to HA
for vm_name in database load-balancer auth-server; do
    vm_id=$(horcrux_list_vms | jq -r ".[] | select(.name == \"$vm_name\") | .id")
    echo "Adding $vm_name to HA..."
    horcrux_add_ha_resource "$vm_id" 255
done

# Check HA status
horcrux_get_ha_status
```

## API Coverage

All examples cover the following Horcrux API features:

### Core Features
- ✅ Virtual Machine management
- ✅ Container management
- ✅ Storage pool management
- ✅ VM snapshots
- ✅ VM cloning
- ✅ Backup and restore

### Advanced Features
- ✅ Clustering and multi-node management
- ✅ High availability (HA)
- ✅ Live migration
- ✅ Monitoring and metrics
- ✅ Alerting
- ✅ Firewall management
- ✅ GPU passthrough
- ✅ Network policies
- ✅ Cloud-init integration
- ✅ Templates
- ✅ Webhooks
- ✅ Audit logging
- ✅ User management and RBAC

## Getting Started

### 1. Choose Your Language

Pick the language that best fits your needs:

- **Python**: Best for complex automation, data processing, integration with other tools
- **Shell**: Best for simple scripts, cron jobs, quick automation

### 2. Install Dependencies

**Python:**
```bash
pip install requests
```

**Shell:**
```bash
# Gentoo
emerge net-misc/curl app-misc/jq

# Debian/Ubuntu
apt-get install curl jq

# macOS
brew install curl jq
```

### 3. Set Up Authentication

**Environment Variables:**
```bash
export HORCRUX_URL="http://localhost:8006"
export HORCRUX_USER="admin"
export HORCRUX_PASS="admin"
```

**Or in code:**
```python
# Python
client = HorcruxClient("http://localhost:8006", "admin", "admin")
```

```bash
# Shell
horcrux_login "http://localhost:8006" "admin" "admin"
```

### 4. Run Examples

**Python:**
```bash
cd python/
python example_basic.py
python example_advanced.py
```

**Shell:**
```bash
cd shell/
./example.sh
```

## Best Practices

### Error Handling

**Python:**
```python
from horcrux_client import HorcruxClient, HorcruxError

try:
    client = HorcruxClient(base_url, username, password)
    vm = client.get_vm("nonexistent-vm")
except HorcruxError as e:
    print(f"Error: {e.message}")
    if e.status_code == 404:
        print("VM not found")
```

**Shell:**
```bash
if ! horcrux_start_vm "$vm_id"; then
    echo "Failed to start VM"
    exit 1
fi
```

### Resource Cleanup

**Python:**
```python
# Use context manager for automatic cleanup
with HorcruxClient(base_url, username, password) as client:
    # Do work
    vms = client.list_vms()
# Automatically logs out

# Or manual cleanup
try:
    client = HorcruxClient(base_url, username, password)
    # Do work
finally:
    client.logout()
```

**Shell:**
```bash
cleanup() {
    horcrux_logout
}
trap cleanup EXIT

horcrux_login "$URL" "$USER" "$PASS"
# Do work
```

### Waiting for Operations

**Python:**
```python
# Wait for VM to be running
client.start_vm(vm_id)
if client.wait_for_vm_status(vm_id, "running", timeout=300):
    print("VM is running")
else:
    print("Timeout")

# Wait for backup to complete
backup = client.create_backup(vm_id)
completed = client.wait_for_backup(backup['id'], timeout=3600)
```

**Shell:**
```bash
# Simple sleep
horcrux_start_vm "$vm_id"
sleep 10

# Poll status
horcrux_start_vm "$vm_id"
while true; do
    status=$(horcrux_get_vm "$vm_id" | jq -r '.status')
    [ "$status" = "running" ] && break
    sleep 5
done
```

## Contributing

We welcome contributions for additional languages and examples!

### Adding a New Language

1. Create a directory: `docs/examples/<language>/`
2. Implement a client library
3. Add basic examples
4. Add advanced examples
5. Write comprehensive README
6. Update this main README

### Improving Examples

- Add more use cases
- Improve error handling
- Add more comments
- Optimize performance
- Add tests

## Testing

Before submitting examples, test them against a running Horcrux instance:

```bash
# Start Horcrux in Docker
cd /path/to/horcrux
docker-compose up -d

# Run examples
cd docs/examples/python
python example_basic.py

cd docs/examples/shell
./example.sh
```

## Documentation

- **API Reference**: [docs/API.md](../API.md)
- **Quick Start**: [docs/QUICKSTART.md](../QUICKSTART.md)
- **Docker Guide**: [docs/DOCKER.md](../DOCKER.md)
- **Website**: https://canutethegreat.github.io/horcrux/

## Support

- **GitHub Issues**: https://github.com/CanuteTheGreat/horcrux/issues
- **Discussions**: https://github.com/CanuteTheGreat/horcrux/discussions

## License

GPL v3 - Same as Horcrux project

---

**Made with ❤️ for the Gentoo community**
