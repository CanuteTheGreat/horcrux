# Horcrux Python Client & Examples

Complete Python client library and examples for the Horcrux virtualization API.

## Installation

### Requirements

- Python 3.7+
- `requests` library

Install dependencies:

```bash
pip install requests
```

### Installing the Client

Copy the client library to your project:

```bash
cp horcrux_client.py /path/to/your/project/
```

Or add it to your Python path:

```bash
export PYTHONPATH="/path/to/horcrux/docs/examples/python:$PYTHONPATH"
```

## Quick Start

```python
from horcrux_client import HorcruxClient

# Initialize client
client = HorcruxClient(
    base_url="http://localhost:8006",
    username="admin",
    password="admin"
)

# List VMs
vms = client.list_vms()
for vm in vms:
    print(f"{vm['name']}: {vm['status']}")

# Create and start a VM
vm = client.create_vm(
    name="my-vm",
    cpus=2,
    memory=2048,
    disk_size=20
)

client.start_vm(vm['id'])

# Cleanup
client.delete_vm(vm['id'])
client.logout()
```

## Using API Keys

```python
# Authenticate with API key instead of username/password
client = HorcruxClient(
    base_url="http://localhost:8006",
    api_key="hx_1234567890abcdef..."
)
```

## Context Manager

```python
# Use context manager for automatic cleanup
with HorcruxClient("http://localhost:8006", "admin", "admin") as client:
    vms = client.list_vms()
    # ... do work ...
# Automatically logs out when done
```

## Examples

### Basic Examples (`example_basic.py`)

Demonstrates fundamental operations:

- Listing VMs
- Creating VMs
- Starting/stopping VMs
- Getting VM details and statistics
- Creating snapshots
- Deleting VMs

Run it:

```bash
python example_basic.py
```

### Advanced Examples (`example_advanced.py`)

Demonstrates advanced features:

- **Backup & Restore**: Creating backups, waiting for completion, restoring
- **High Availability**: Adding VMs to HA, configuring failover
- **Clustering**: Managing cluster nodes, finding optimal placement
- **Monitoring**: Getting statistics, historical metrics, creating alerts
- **Snapshots & Cloning**: Multiple snapshots, full/linked clones
- **Firewall**: Adding rules, applying policies

Run it:

```bash
python example_advanced.py
```

## API Coverage

The client library covers all major Horcrux API endpoints:

### Virtual Machines
- `list_vms()` - List all VMs
- `get_vm(vm_id)` - Get VM details
- `create_vm(...)` - Create a new VM
- `start_vm(vm_id)` - Start a VM
- `stop_vm(vm_id, force, timeout)` - Stop a VM
- `delete_vm(vm_id, purge)` - Delete a VM

### VM Snapshots
- `list_snapshots(vm_id)` - List VM snapshots
- `create_snapshot(vm_id, name, ...)` - Create snapshot
- `restore_snapshot(vm_id, snapshot_id)` - Restore snapshot
- `delete_snapshot(vm_id, snapshot_id)` - Delete snapshot

### VM Cloning
- `clone_vm(vm_id, new_name, ...)` - Clone a VM
- `clone_vm_cross_node(...)` - Clone to another node
- `list_clone_jobs()` - List clone jobs
- `get_clone_job(job_id)` - Get clone job status

### Containers
- `list_containers()` - List all containers
- `create_container(...)` - Create a container
- `start_container(container_id)` - Start container
- `stop_container(container_id)` - Stop container
- `delete_container(container_id)` - Delete container
- `exec_in_container(container_id, command)` - Execute command

### Storage
- `list_storage_pools()` - List storage pools
- `get_storage_pool(pool_id)` - Get pool details
- `add_storage_pool(name, type, config)` - Add storage pool
- `remove_storage_pool(pool_id)` - Remove storage pool

### Backups
- `list_backups(vm_id)` - List backups
- `create_backup(vm_id, ...)` - Create backup
- `restore_backup(backup_id, target_vm_id)` - Restore backup
- `delete_backup(backup_id)` - Delete backup
- `wait_for_backup(backup_id, timeout)` - Wait for completion

### Clustering
- `list_cluster_nodes()` - List cluster nodes
- `add_cluster_node(name, address, arch)` - Add node
- `get_cluster_architecture()` - Get architecture summary

### High Availability
- `list_ha_resources()` - List HA resources
- `add_ha_resource(vm_id, priority, group)` - Add to HA
- `remove_ha_resource(vm_id)` - Remove from HA
- `get_ha_status()` - Get HA status

### Migration
- `migrate_vm(vm_id, target_node, ...)` - Migrate VM
- `get_migration_status(vm_id)` - Get migration status

### Monitoring
- `get_node_stats()` - Get node statistics
- `get_vm_stats(vm_id)` - Get VM statistics
- `get_all_vm_stats()` - Get all VM statistics
- `get_metric_history(metric, start, end, interval)` - Get historical data

### Alerts
- `list_alert_rules()` - List alert rules
- `create_alert_rule(...)` - Create alert rule
- `list_active_alerts()` - List active alerts
- `acknowledge_alert(rule_id, target, comment)` - Acknowledge alert

### Firewall
- `list_firewall_rules()` - List firewall rules
- `add_firewall_rule(...)` - Add firewall rule
- `delete_firewall_rule(rule_id)` - Delete firewall rule
- `apply_firewall_rules(scope)` - Apply rules

### GPU Passthrough
- `list_gpu_devices()` - List GPU devices
- `scan_gpu_devices()` - Scan for GPUs
- `bind_gpu_to_vfio(pci_address)` - Bind to VFIO
- `unbind_gpu_from_vfio(pci_address)` - Unbind from VFIO
- `check_iommu_status()` - Check IOMMU status

### Helper Methods
- `wait_for_vm_status(vm_id, status, timeout)` - Wait for VM status
- `wait_for_backup(backup_id, timeout)` - Wait for backup completion

## Error Handling

All API errors raise `HorcruxError`:

```python
from horcrux_client import HorcruxClient, HorcruxError

client = HorcruxClient("http://localhost:8006", "admin", "admin")

try:
    vm = client.get_vm("nonexistent-vm")
except HorcruxError as e:
    print(f"Error: {e.message}")
    print(f"Status code: {e.status_code}")
    print(f"Response: {e.response}")
```

## Advanced Usage

### Custom Timeout

```python
# Create client with custom SSL verification
client = HorcruxClient(
    base_url="https://horcrux.example.com",
    username="admin",
    password="admin",
    verify_ssl=False  # Disable SSL verification (not recommended for production!)
)
```

### Waiting for Operations

```python
# Wait for VM to reach a specific status
vm = client.create_vm(name="my-vm", cpus=2, memory=2048, disk_size=20)
client.start_vm(vm['id'])

# Wait up to 5 minutes for VM to be running
if client.wait_for_vm_status(vm['id'], "running", timeout=300):
    print("VM is running!")
else:
    print("Timeout waiting for VM to start")
```

### Batch Operations

```python
# Create multiple VMs
vms = []
for i in range(10):
    vm = client.create_vm(
        name=f"vm-{i}",
        cpus=2,
        memory=2048,
        disk_size=20
    )
    vms.append(vm)
    print(f"Created VM {i+1}/10")

# Start all VMs
for vm in vms:
    client.start_vm(vm['id'])

# Wait for all to be running
for vm in vms:
    client.wait_for_vm_status(vm['id'], "running")

print("All VMs are running!")
```

### Monitoring Loop

```python
import time

# Monitor node stats every 10 seconds
while True:
    stats = client.get_node_stats()
    print(f"CPU: {stats['cpu_usage']}%, "
          f"Memory: {stats['memory_used']/stats['memory_total']*100:.1f}%")
    time.sleep(10)
```

## Complete Example: Deploy Multi-Tier Application

```python
from horcrux_client import HorcruxClient

client = HorcruxClient("http://localhost:8006", "admin", "admin")

# Create database VM
db_vm = client.create_vm(
    name="database",
    cpus=4,
    memory=8192,
    disk_size=100,
    hypervisor="Qemu"
)
client.start_vm(db_vm['id'])

# Create web server VMs (3 replicas)
web_vms = []
for i in range(3):
    vm = client.create_vm(
        name=f"web-{i+1}",
        cpus=2,
        memory=4096,
        disk_size=50
    )
    client.start_vm(vm['id'])
    web_vms.append(vm)

# Add database to HA for automatic failover
client.add_ha_resource(
    vm_id=db_vm['id'],
    priority=255,  # Highest priority
    group="production"
)

# Create firewall rules for web servers
for vm in web_vms:
    # Allow HTTP
    client.add_firewall_rule(
        name=f"allow-http-{vm['id']}",
        action="Accept",
        protocol="Tcp",
        port=80
    )

    # Allow HTTPS
    client.add_firewall_rule(
        name=f"allow-https-{vm['id']}",
        action="Accept",
        protocol="Tcp",
        port=443
    )

# Apply firewall rules
client.apply_firewall_rules(scope="datacenter")

# Create alert for high CPU on database
client.create_alert_rule(
    name="Database High CPU",
    metric="cpu_usage",
    threshold=80.0,
    condition="greater_than",
    severity="critical",
    target_type="vm",
    target_id=db_vm['id']
)

# Create nightly backup job
client.create_backup(
    vm_id=db_vm['id'],
    backup_type="full",
    compression="zstd",
    description="Nightly database backup"
)

print("Multi-tier application deployed successfully!")
print(f"Database VM: {db_vm['id']}")
print(f"Web VMs: {[vm['id'] for vm in web_vms]}")

client.logout()
```

## Testing

To test the client against a running Horcrux instance:

```bash
# Set environment variables
export HORCRUX_URL="http://localhost:8006"
export HORCRUX_USER="admin"
export HORCRUX_PASS="admin"

# Run basic examples
python example_basic.py

# Run advanced examples
python example_advanced.py
```

## Contributing

Contributions are welcome! Please ensure:

1. Code follows PEP 8 style guidelines
2. All methods have docstrings
3. Error handling is implemented
4. Examples are updated for new features

## License

GPL v3 - Same as Horcrux project

## Support

- **Documentation**: https://canutethegreat.github.io/horcrux/
- **API Reference**: https://github.com/CanuteTheGreat/horcrux/blob/main/docs/API.md
- **Issues**: https://github.com/CanuteTheGreat/horcrux/issues
