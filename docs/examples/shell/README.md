# Horcrux Shell Client

Simple shell script wrapper for the Horcrux API using `curl` and `jq`.

## Prerequisites

- `bash` 4.0+
- `curl`
- `jq` (for JSON parsing)

Install on Gentoo:
```bash
emerge -av net-misc/curl app-misc/jq
```

Install on Debian/Ubuntu:
```bash
apt-get install curl jq
```

Install on macOS:
```bash
brew install curl jq
```

## Quick Start

### Interactive Usage

```bash
# Source the client library
source horcrux.sh

# Login
horcrux_login "http://localhost:8006" "admin" "admin"

# List VMs
horcrux_list_vms

# Create a VM
horcrux_create_vm "my-vm" 2 2048 20

# Start the VM
horcrux_start_vm "vm-100"

# Get VM stats
horcrux_get_vm_stats "vm-100"

# Logout
horcrux_logout
```

### Script Usage

```bash
#!/bin/bash
source /path/to/horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Your automation here
horcrux_list_vms
horcrux_create_backup "vm-100" "full" "zstd"

horcrux_logout
```

## Running the Example

```bash
# Set environment variables (optional)
export HORCRUX_URL="http://localhost:8006"
export HORCRUX_USER="admin"
export HORCRUX_PASS="admin"

# Run the example
./example.sh
```

## Available Functions

### Authentication

- `horcrux_login <url> <username> <password>` - Login to Horcrux
- `horcrux_logout` - Logout and invalidate session

### Virtual Machines

- `horcrux_list_vms` - List all VMs
- `horcrux_get_vm <vm_id>` - Get VM details
- `horcrux_create_vm <name> [cpus] [memory] [disk_size] [hypervisor]` - Create VM
- `horcrux_start_vm <vm_id>` - Start a VM
- `horcrux_stop_vm <vm_id> [force]` - Stop a VM
- `horcrux_delete_vm <vm_id>` - Delete a VM

### Snapshots

- `horcrux_list_snapshots <vm_id>` - List VM snapshots
- `horcrux_create_snapshot <vm_id> <name> [description]` - Create snapshot
- `horcrux_restore_snapshot <vm_id> <snapshot_id>` - Restore snapshot
- `horcrux_delete_snapshot <vm_id> <snapshot_id>` - Delete snapshot

### Cloning

- `horcrux_clone_vm <vm_id> <new_name> [full_clone]` - Clone a VM

### Containers

- `horcrux_list_containers` - List all containers
- `horcrux_create_container <name> [runtime] [image] [cpus] [memory]` - Create container
- `horcrux_start_container <container_id>` - Start container
- `horcrux_stop_container <container_id>` - Stop container
- `horcrux_delete_container <container_id>` - Delete container

### Backups

- `horcrux_list_backups [vm_id]` - List backups
- `horcrux_create_backup <vm_id> [type] [compression]` - Create backup
- `horcrux_restore_backup <backup_id> [target_vm_id]` - Restore backup

### Storage

- `horcrux_list_storage_pools` - List storage pools
- `horcrux_get_storage_pool <pool_id>` - Get pool details

### Clustering

- `horcrux_list_cluster_nodes` - List cluster nodes
- `horcrux_get_cluster_architecture` - Get cluster architecture

### Monitoring

- `horcrux_get_node_stats` - Get node statistics
- `horcrux_get_vm_stats <vm_id>` - Get VM statistics
- `horcrux_get_all_vm_stats` - Get all VM statistics

### Alerts

- `horcrux_list_alert_rules` - List alert rules
- `horcrux_list_active_alerts` - List active alerts

### High Availability

- `horcrux_list_ha_resources` - List HA resources
- `horcrux_get_ha_status` - Get HA status
- `horcrux_add_ha_resource <vm_id> [priority]` - Add VM to HA

### Firewall

- `horcrux_list_firewall_rules` - List firewall rules
- `horcrux_add_firewall_rule <name> <action> <protocol> <port>` - Add rule
- `horcrux_apply_firewall_rules [scope]` - Apply firewall rules

### GPU

- `horcrux_list_gpu_devices` - List GPU devices
- `horcrux_check_iommu_status` - Check IOMMU status

### Utility

- `horcrux_health` - Check API health
- `horcrux_help` - Show help message

## Examples

### Create and Configure a VM

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Create VM
vm_output=$(horcrux_create_vm "web-server" 4 8192 100 "Qemu")
vm_id=$(echo "$vm_output" | jq -r '.id')

# Start VM
horcrux_start_vm "$vm_id"

# Wait a bit
sleep 10

# Create initial snapshot
horcrux_create_snapshot "$vm_id" "initial" "Initial configuration"

# Add firewall rules
horcrux_add_firewall_rule "allow-http" "Accept" "Tcp" 80
horcrux_add_firewall_rule "allow-https" "Accept" "Tcp" 443
horcrux_apply_firewall_rules "datacenter"

# Add to HA
horcrux_add_ha_resource "$vm_id" 200

horcrux_logout
```

### Automated Backups

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Get all running VMs
vms=$(horcrux_api_call "GET" "/api/vms")

# Backup each VM
echo "$vms" | jq -r '.[] | select(.status == "running") | .id' | while read vm_id; do
    echo "Backing up $vm_id..."
    horcrux_create_backup "$vm_id" "full" "zstd"
done

horcrux_logout
```

### Monitoring Script

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Get node stats
echo "=== Node Statistics ==="
stats=$(horcrux_get_node_stats)
echo "$stats" | jq -r '"CPU: \(.cpu_usage)%, Memory: \(.memory_usage)%"'

# Get all VM stats
echo ""
echo "=== VM Statistics ==="
horcrux_get_all_vm_stats | jq -r '.[] | "\(.vm_id): CPU \(.cpu_usage)%, Memory \(.memory_usage)%"'

# Check active alerts
echo ""
echo "=== Active Alerts ==="
alerts=$(horcrux_list_active_alerts)
alert_count=$(echo "$alerts" | jq 'length')
echo "Active alerts: $alert_count"

if [ "$alert_count" -gt 0 ]; then
    echo "$alerts" | jq -r '.[] | "  - \(.rule_name): \(.severity)"'
fi

horcrux_logout
```

### Clone Multiple VMs

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Template VM
TEMPLATE_VM="template-vm"

# Create 5 clones
for i in {1..5}; do
    echo "Creating clone $i..."
    horcrux_clone_vm "$TEMPLATE_VM" "web-server-$i" true

    # Start the clone
    horcrux_start_vm "web-server-$i"
done

horcrux_logout
```

### Cluster Health Check

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

echo "=== Cluster Health Check ==="
echo ""

# List nodes
echo "Cluster Nodes:"
nodes=$(horcrux_list_cluster_nodes)
echo "$nodes" | jq -r '.[] | "\(.name): \(.status) - \(.vms_running) VMs running"'

# Get architecture
echo ""
echo "Cluster Architecture:"
horcrux_get_cluster_architecture | jq '.'

# Get HA status
echo ""
echo "HA Status:"
horcrux_get_ha_status | jq '.'

horcrux_logout
```

### Batch VM Operations

```bash
#!/bin/bash
source horcrux.sh

horcrux_login "http://localhost:8006" "admin" "admin"

# Get all VMs
vms=$(horcrux_list_vms)

# Stop all VMs with a specific prefix
echo "$vms" | jq -r '.[] | select(.name | startswith("test-")) | .id' | while read vm_id; do
    echo "Stopping $vm_id..."
    horcrux_stop_vm "$vm_id" true
done

# Delete them after stopping
sleep 5

echo "$vms" | jq -r '.[] | select(.name | startswith("test-")) | .id' | while read vm_id; do
    echo "Deleting $vm_id..."
    horcrux_delete_vm "$vm_id"
done

horcrux_logout
```

## Environment Variables

- `HORCRUX_URL` - Default Horcrux API URL
- `HORCRUX_USER` - Default username
- `HORCRUX_PASS` - Default password

## Tips

### Pretty Printing

All functions return JSON. Use `jq` for pretty printing and filtering:

```bash
# Get only VM names
horcrux_list_vms | jq -r '.[].name'

# Get VMs with high CPU
horcrux_get_all_vm_stats | jq '.[] | select(.cpu_usage > 80)'

# Count running VMs
horcrux_list_vms | jq '[.[] | select(.status == "running")] | length'
```

### Error Handling

```bash
if horcrux_start_vm "$vm_id"; then
    echo "VM started successfully"
else
    echo "Failed to start VM"
    exit 1
fi
```

### Scripting Best Practices

```bash
#!/bin/bash
set -euo pipefail  # Exit on error, undefined vars, pipe failures

source horcrux.sh

cleanup() {
    horcrux_logout
}
trap cleanup EXIT

horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

# Your script here
```

## Troubleshooting

### jq not found

Install jq:
```bash
# Gentoo
emerge app-misc/jq

# Debian/Ubuntu
apt-get install jq

# macOS
brew install jq
```

### Connection refused

Check if Horcrux is running:
```bash
curl http://localhost:8006/api/health
```

### Authentication failed

Verify credentials:
```bash
horcrux_login "http://localhost:8006" "admin" "admin"
```

## License

GPL v3 - Same as Horcrux project

## Support

- **Documentation**: https://canutethegreat.github.io/horcrux/
- **API Reference**: https://github.com/CanuteTheGreat/horcrux/blob/main/docs/API.md
- **Issues**: https://github.com/CanuteTheGreat/horcrux/issues
