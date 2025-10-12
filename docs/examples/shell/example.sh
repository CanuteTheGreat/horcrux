#!/bin/bash
#
# Horcrux Shell Example
#
# Demonstrates common operations using the shell client.
#

set -e

# Load the Horcrux shell client
source "$(dirname "$0")/horcrux.sh"

# Configuration
HORCRUX_URL="${HORCRUX_URL:-http://localhost:8006}"
HORCRUX_USER="${HORCRUX_USER:-admin}"
HORCRUX_PASS="${HORCRUX_PASS:-admin}"

# ========================================
# Main Script
# ========================================

main() {
    echo "======================================================================"
    echo "Horcrux Shell Client Example"
    echo "======================================================================"

    # 1. Login
    echo ""
    echo "1. Logging in..."
    horcrux_login "$HORCRUX_URL" "$HORCRUX_USER" "$HORCRUX_PASS"

    # 2. Check health
    echo ""
    echo "2. Checking API health..."
    horcrux_health

    # 3. List existing VMs
    echo ""
    echo "3. Listing existing VMs..."
    horcrux_list_vms

    # 4. Create a new VM
    echo ""
    echo "4. Creating new VM..."
    VM_NAME="shell-example-vm"
    create_output=$(horcrux_create_vm "$VM_NAME" 2 2048 20)
    VM_ID=$(echo "$create_output" | jq -r '.id')
    echo "   VM ID: $VM_ID"

    # 5. Start the VM
    echo ""
    echo "5. Starting VM..."
    horcrux_start_vm "$VM_ID"
    sleep 5  # Give it a moment

    # 6. Get VM details
    echo ""
    echo "6. Getting VM details..."
    horcrux_get_vm "$VM_ID"

    # 7. Get VM statistics
    echo ""
    echo "7. Getting VM statistics..."
    horcrux_get_vm_stats "$VM_ID"

    # 8. Create a snapshot
    echo ""
    echo "8. Creating snapshot..."
    horcrux_create_snapshot "$VM_ID" "initial-snapshot" "First snapshot"

    # 9. List snapshots
    echo ""
    echo "9. Listing snapshots..."
    horcrux_list_snapshots "$VM_ID"

    # 10. Create a backup
    echo ""
    echo "10. Creating backup..."
    horcrux_create_backup "$VM_ID" "full" "zstd"

    # 11. List backups
    echo ""
    echo "11. Listing backups..."
    horcrux_list_backups "$VM_ID"

    # 12. Clone the VM
    echo ""
    echo "12. Cloning VM..."
    clone_output=$(horcrux_clone_vm "$VM_ID" "${VM_NAME}-clone" true)
    CLONE_ID=$(echo "$clone_output" | jq -r '.id')
    echo "   Clone ID: $CLONE_ID"

    # 13. Get node statistics
    echo ""
    echo "13. Getting node statistics..."
    horcrux_get_node_stats

    # 14. List storage pools
    echo ""
    echo "14. Listing storage pools..."
    horcrux_list_storage_pools

    # 15. List cluster nodes
    echo ""
    echo "15. Listing cluster nodes..."
    horcrux_list_cluster_nodes

    # 16. Cleanup
    echo ""
    echo "16. Cleaning up..."

    echo "   Stopping VMs..."
    horcrux_stop_vm "$VM_ID" true > /dev/null
    horcrux_stop_vm "$CLONE_ID" true > /dev/null
    sleep 3

    echo "   Deleting VMs..."
    horcrux_delete_vm "$VM_ID" > /dev/null
    horcrux_delete_vm "$CLONE_ID" > /dev/null

    # 17. Logout
    echo ""
    echo "17. Logging out..."
    horcrux_logout

    echo ""
    echo "======================================================================"
    echo "Example completed successfully!"
    echo "======================================================================"
}

# Run the main script
main "$@"
