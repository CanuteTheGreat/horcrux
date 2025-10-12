#!/usr/bin/env python3
"""
Basic Horcrux API Examples

Demonstrates basic VM management operations using the Horcrux Python client.
"""

from horcrux_client import HorcruxClient, HorcruxError
import sys


def main():
    # Initialize client
    client = HorcruxClient(
        base_url="http://localhost:8006",
        username="admin",
        password="admin"
    )

    print("=" * 60)
    print("Horcrux Basic Examples")
    print("=" * 60)

    try:
        # Example 1: List existing VMs
        print("\n1. Listing all VMs...")
        vms = client.list_vms()
        print(f"   Found {len(vms)} VMs:")
        for vm in vms:
            print(f"   - {vm['name']} ({vm['id']}): {vm['status']}")

        # Example 2: Create a new VM
        print("\n2. Creating a new VM...")
        vm = client.create_vm(
            name="example-vm",
            cpus=2,
            memory=2048,  # 2GB
            disk_size=20,  # 20GB
            hypervisor="Qemu",
            architecture="X86_64"
        )
        vm_id = vm['id']
        print(f"   ✓ Created VM '{vm['name']}' with ID: {vm_id}")

        # Example 3: Start the VM
        print("\n3. Starting the VM...")
        client.start_vm(vm_id)
        print("   ✓ VM start command sent")

        # Wait for VM to be running
        print("   Waiting for VM to start...", end="", flush=True)
        if client.wait_for_vm_status(vm_id, "running", timeout=60):
            print(" ✓")
        else:
            print(" ✗ (timeout)")

        # Example 4: Get VM details
        print("\n4. Getting VM details...")
        vm_details = client.get_vm(vm_id)
        print(f"   Name: {vm_details['name']}")
        print(f"   Status: {vm_details['status']}")
        print(f"   CPUs: {vm_details['cpus']}")
        print(f"   Memory: {vm_details['memory']} MB")
        print(f"   Hypervisor: {vm_details['hypervisor']}")

        # Example 5: Get VM statistics
        print("\n5. Getting VM statistics...")
        stats = client.get_vm_stats(vm_id)
        print(f"   CPU Usage: {stats.get('cpu_usage', 'N/A')}%")
        print(f"   Memory Usage: {stats.get('memory_usage', 'N/A')}%")
        print(f"   Uptime: {stats.get('uptime_seconds', 0)} seconds")

        # Example 6: Create a snapshot
        print("\n6. Creating a snapshot...")
        snapshot = client.create_snapshot(
            vm_id=vm_id,
            name="initial-snapshot",
            description="First snapshot after creation"
        )
        print(f"   ✓ Created snapshot '{snapshot['name']}'")

        # Example 7: List snapshots
        print("\n7. Listing snapshots...")
        snapshots = client.list_snapshots(vm_id)
        print(f"   Found {len(snapshots)} snapshot(s):")
        for snap in snapshots:
            print(f"   - {snap['name']} (created: {snap['created_at']})")

        # Example 8: Stop the VM
        print("\n8. Stopping the VM...")
        client.stop_vm(vm_id, force=False, timeout=60)
        print("   ✓ VM stop command sent")

        # Wait for VM to be stopped
        print("   Waiting for VM to stop...", end="", flush=True)
        if client.wait_for_vm_status(vm_id, "stopped", timeout=60):
            print(" ✓")
        else:
            print(" ✗ (timeout)")

        # Example 9: Delete the VM
        print("\n9. Deleting the VM...")
        confirm = input("   Delete the VM? (y/N): ")
        if confirm.lower() == 'y':
            client.delete_vm(vm_id, purge=True)
            print("   ✓ VM deleted")
        else:
            print("   Skipped deletion")

        print("\n" + "=" * 60)
        print("All examples completed successfully!")
        print("=" * 60)

    except HorcruxError as e:
        print(f"\n✗ Error: {e.message}")
        if e.status_code:
            print(f"  Status code: {e.status_code}")
        sys.exit(1)

    finally:
        # Cleanup
        client.logout()


if __name__ == "__main__":
    main()
