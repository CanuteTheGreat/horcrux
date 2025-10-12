#!/usr/bin/env python3
"""
Advanced Horcrux API Examples

Demonstrates advanced features: clustering, HA, backups, monitoring, and more.
"""

from horcrux_client import HorcruxClient, HorcruxError
from datetime import datetime, timedelta
import sys
import time


def example_backup_and_restore(client: HorcruxClient, vm_id: str):
    """Example: Backup and restore operations"""
    print("\n" + "=" * 60)
    print("Backup & Restore Example")
    print("=" * 60)

    # Create a backup
    print("\n1. Creating backup...")
    backup = client.create_backup(
        vm_id=vm_id,
        backup_type="full",
        compression="zstd",
        description="Automated weekly backup"
    )
    backup_id = backup['id']
    print(f"   ✓ Backup started: {backup_id}")

    # Wait for backup to complete
    print("   Waiting for backup to complete...")
    try:
        completed_backup = client.wait_for_backup(backup_id, timeout=600)
        print(f"   ✓ Backup completed: {completed_backup['size']} bytes")
    except HorcruxError as e:
        print(f"   ✗ Backup failed: {e.message}")
        return

    # List all backups
    print("\n2. Listing all backups for VM...")
    backups = client.list_backups(vm_id=vm_id)
    for b in backups:
        print(f"   - {b['id']}: {b['type']}, {b['status']}")

    # Restore backup (to a new VM)
    print("\n3. Restoring backup to new VM...")
    restored_vm_id = f"{vm_id}-restored"
    client.restore_backup(backup_id, target_vm_id=restored_vm_id)
    print(f"   ✓ Restored to VM: {restored_vm_id}")

    # Cleanup
    print("\n4. Cleaning up restored VM...")
    client.delete_vm(restored_vm_id)
    print("   ✓ Cleanup complete")


def example_high_availability(client: HorcruxClient, vm_id: str):
    """Example: High availability configuration"""
    print("\n" + "=" * 60)
    print("High Availability Example")
    print("=" * 60)

    # Add VM to HA management
    print("\n1. Adding VM to HA...")
    client.add_ha_resource(
        vm_id=vm_id,
        priority=100,
        group="critical-services"
    )
    print("   ✓ VM added to HA management")

    # List HA resources
    print("\n2. Listing HA resources...")
    ha_resources = client.list_ha_resources()
    for resource in ha_resources:
        print(f"   - VM {resource['vm_id']}: priority={resource['priority']}, "
              f"state={resource['state']}")

    # Get HA status
    print("\n3. Getting HA status...")
    ha_status = client.get_ha_status()
    print(f"   Enabled: {ha_status['enabled']}")
    print(f"   Total resources: {ha_status['total_resources']}")
    print(f"   Running: {ha_status['resources_started']}")

    # Remove from HA
    print("\n4. Removing VM from HA...")
    client.remove_ha_resource(vm_id)
    print("   ✓ VM removed from HA management")


def example_cluster_operations(client: HorcruxClient):
    """Example: Cluster management"""
    print("\n" + "=" * 60)
    print("Cluster Management Example")
    print("=" * 60)

    # List cluster nodes
    print("\n1. Listing cluster nodes...")
    nodes = client.list_cluster_nodes()
    print(f"   Found {len(nodes)} node(s):")
    for node in nodes:
        print(f"   - {node['name']}: {node['architecture']}, "
              f"status={node['status']}, VMs={node.get('vms_running', 0)}")

    # Get cluster architecture
    print("\n2. Getting cluster architecture...")
    arch = client.get_cluster_architecture()
    print(f"   Total nodes: {arch['total_nodes']}")
    print("   Architectures:")
    for arch_name, count in arch['architectures'].items():
        print(f"   - {arch_name}: {count} node(s)")


def example_monitoring_and_alerts(client: HorcruxClient, vm_id: str):
    """Example: Monitoring and alerting"""
    print("\n" + "=" * 60)
    print("Monitoring & Alerting Example")
    print("=" * 60)

    # Get node statistics
    print("\n1. Getting node statistics...")
    node_stats = client.get_node_stats()
    print(f"   CPU Usage: {node_stats['cpu_usage']}%")
    print(f"   Memory: {node_stats['memory_used']/node_stats['memory_total']*100:.1f}%")
    print(f"   Uptime: {node_stats['uptime_seconds']/3600:.1f} hours")

    # Get VM statistics
    print("\n2. Getting VM statistics...")
    vm_stats = client.get_vm_stats(vm_id)
    print(f"   CPU Usage: {vm_stats.get('cpu_usage', 'N/A')}%")
    print(f"   Memory Usage: {vm_stats.get('memory_usage', 'N/A')}%")
    print(f"   Network RX: {vm_stats.get('network_rx_bytes', 0)/1024/1024:.2f} MB")
    print(f"   Network TX: {vm_stats.get('network_tx_bytes', 0)/1024/1024:.2f} MB")

    # Get metric history
    print("\n3. Getting CPU usage history (last hour)...")
    end_time = datetime.now()
    start_time = end_time - timedelta(hours=1)

    history = client.get_metric_history(
        metric="cpu_usage",
        start=start_time,
        end=end_time,
        interval=300  # 5 minutes
    )

    if history.get('data'):
        print(f"   Retrieved {len(history['data'])} data points")
        # Show first and last
        if len(history['data']) > 0:
            first = history['data'][0]
            last = history['data'][-1]
            print(f"   First: {first['timestamp']} = {first['value']}%")
            print(f"   Last: {last['timestamp']} = {last['value']}%")

    # Create an alert rule
    print("\n4. Creating alert rule...")
    alert_rule = client.create_alert_rule(
        name="High CPU Alert",
        metric="cpu_usage",
        threshold=80.0,
        condition="greater_than",
        severity="warning",
        target_type="vm",
        target_id=vm_id
    )
    print(f"   ✓ Created alert rule: {alert_rule['id']}")

    # List alert rules
    print("\n5. Listing alert rules...")
    rules = client.list_alert_rules()
    for rule in rules:
        print(f"   - {rule['name']}: {rule['condition']} {rule['threshold']}")

    # List active alerts
    print("\n6. Checking active alerts...")
    active_alerts = client.list_active_alerts()
    if active_alerts:
        print(f"   Found {len(active_alerts)} active alert(s)")
        for alert in active_alerts:
            print(f"   - {alert['rule_name']}: {alert['severity']}")
    else:
        print("   No active alerts")


def example_snapshots_and_clones(client: HorcruxClient, vm_id: str):
    """Example: Advanced snapshot and cloning operations"""
    print("\n" + "=" * 60)
    print("Snapshots & Cloning Example")
    print("=" * 60)

    # Create multiple snapshots
    print("\n1. Creating multiple snapshots...")
    snapshots = []
    for i in range(3):
        snapshot = client.create_snapshot(
            vm_id=vm_id,
            name=f"snapshot-{i+1}",
            description=f"Test snapshot {i+1}"
        )
        snapshots.append(snapshot)
        print(f"   ✓ Created snapshot {i+1}")
        time.sleep(1)  # Small delay between snapshots

    # List all snapshots
    print("\n2. Listing all snapshots...")
    all_snapshots = client.list_snapshots(vm_id)
    print(f"   Total: {len(all_snapshots)} snapshots")

    # Clone VM (full clone)
    print("\n3. Creating full clone...")
    clone = client.clone_vm(
        vm_id=vm_id,
        new_name=f"{vm_id}-clone",
        full_clone=True,
        start=False
    )
    clone_id = clone['id']
    print(f"   ✓ Cloned VM: {clone_id}")

    # Restore to snapshot
    print("\n4. Restoring to first snapshot...")
    client.stop_vm(vm_id, force=False)
    client.wait_for_vm_status(vm_id, "stopped", timeout=60)

    client.restore_snapshot(vm_id, snapshots[0]['id'])
    print("   ✓ Restored to snapshot")

    # Cleanup
    print("\n5. Cleaning up...")
    print("   Deleting snapshots...")
    for snap in snapshots:
        client.delete_snapshot(vm_id, snap['id'])
    print("   ✓ Deleted snapshots")

    print("   Deleting clone...")
    client.delete_vm(clone_id)
    print("   ✓ Deleted clone")


def example_firewall(client: HorcruxClient):
    """Example: Firewall management"""
    print("\n" + "=" * 60)
    print("Firewall Management Example")
    print("=" * 60)

    # Add firewall rules
    print("\n1. Adding firewall rules...")

    # SSH rule
    ssh_rule = client.add_firewall_rule(
        name="allow-ssh",
        action="Accept",
        protocol="Tcp",
        port=22,
        source="0.0.0.0/0",
        enabled=True
    )
    print(f"   ✓ Added SSH rule: {ssh_rule['id']}")

    # HTTP rule
    http_rule = client.add_firewall_rule(
        name="allow-http",
        action="Accept",
        protocol="Tcp",
        port=80,
        source="0.0.0.0/0",
        enabled=True
    )
    print(f"   ✓ Added HTTP rule: {http_rule['id']}")

    # HTTPS rule
    https_rule = client.add_firewall_rule(
        name="allow-https",
        action="Accept",
        protocol="Tcp",
        port=443,
        source="0.0.0.0/0",
        enabled=True
    )
    print(f"   ✓ Added HTTPS rule: {https_rule['id']}")

    # List all rules
    print("\n2. Listing firewall rules...")
    rules = client.list_firewall_rules()
    for rule in rules:
        print(f"   - {rule['name']}: {rule['action']} {rule['protocol']} "
              f"port {rule.get('port', 'any')}")

    # Apply rules
    print("\n3. Applying firewall rules...")
    result = client.apply_firewall_rules(scope="datacenter")
    print(f"   ✓ Applied {result['rules_applied']} rules")

    # Cleanup (delete rules we created)
    print("\n4. Cleaning up firewall rules...")
    for rule_id in [ssh_rule['id'], http_rule['id'], https_rule['id']]:
        client.delete_firewall_rule(rule_id)
    print("   ✓ Rules deleted")


def main():
    # Initialize client
    client = HorcruxClient(
        base_url="http://localhost:8006",
        username="admin",
        password="admin"
    )

    print("=" * 60)
    print("Horcrux Advanced Examples")
    print("=" * 60)

    try:
        # Create a test VM for demonstrations
        print("\nSetting up test VM...")
        vm = client.create_vm(
            name="advanced-example-vm",
            cpus=2,
            memory=2048,
            disk_size=20,
            hypervisor="Qemu"
        )
        vm_id = vm['id']
        print(f"✓ Created test VM: {vm_id}")

        # Start the VM
        client.start_vm(vm_id)
        print("✓ Starting VM...")
        client.wait_for_vm_status(vm_id, "running", timeout=60)

        # Run examples
        example_cluster_operations(client)
        example_monitoring_and_alerts(client, vm_id)
        example_snapshots_and_clones(client, vm_id)
        example_high_availability(client, vm_id)
        example_backup_and_restore(client, vm_id)
        example_firewall(client)

        # Cleanup
        print("\n" + "=" * 60)
        print("Cleaning up...")
        print("=" * 60)

        client.stop_vm(vm_id, force=True)
        client.wait_for_vm_status(vm_id, "stopped", timeout=60)
        client.delete_vm(vm_id)
        print("✓ Test VM deleted")

        print("\n" + "=" * 60)
        print("All advanced examples completed successfully!")
        print("=" * 60)

    except HorcruxError as e:
        print(f"\n✗ Error: {e.message}")
        if e.status_code:
            print(f"  Status code: {e.status_code}")
        sys.exit(1)

    finally:
        client.logout()


if __name__ == "__main__":
    main()
