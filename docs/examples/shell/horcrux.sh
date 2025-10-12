#!/bin/bash
#
# Horcrux Shell Client
#
# A simple shell script wrapper for the Horcrux API using curl and jq.
# Provides easy command-line access to common Horcrux operations.
#
# Requirements:
#   - curl
#   - jq (for JSON parsing)
#
# Usage:
#   source horcrux.sh
#   horcrux_login "http://localhost:8006" "admin" "admin"
#   horcrux_list_vms
#   horcrux_create_vm "my-vm" 2 2048 20
#

set -e

# Global variables
HORCRUX_BASE_URL=""
HORCRUX_TOKEN=""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ========================================
# Helper Functions
# ========================================

horcrux_log() {
    echo -e "${GREEN}[Horcrux]${NC} $1"
}

horcrux_error() {
    echo -e "${RED}[Error]${NC} $1" >&2
}

horcrux_warn() {
    echo -e "${YELLOW}[Warning]${NC} $1"
}

horcrux_check_deps() {
    if ! command -v curl &> /dev/null; then
        horcrux_error "curl is required but not installed"
        return 1
    fi

    if ! command -v jq &> /dev/null; then
        horcrux_warn "jq is not installed - JSON output will not be formatted"
    fi
}

horcrux_api_call() {
    local method="$1"
    local path="$2"
    local data="$3"

    local url="${HORCRUX_BASE_URL}${path}"
    local response
    local http_code

    if [ -z "$data" ]; then
        response=$(curl -s -w "\n%{http_code}" -X "$method" \
            -H "Authorization: Bearer $HORCRUX_TOKEN" \
            -H "Content-Type: application/json" \
            "$url")
    else
        response=$(curl -s -w "\n%{http_code}" -X "$method" \
            -H "Authorization: Bearer $HORCRUX_TOKEN" \
            -H "Content-Type: application/json" \
            -d "$data" \
            "$url")
    fi

    http_code=$(echo "$response" | tail -n1)
    body=$(echo "$response" | sed '$d')

    if [ "$http_code" -ge 400 ]; then
        horcrux_error "API call failed with status $http_code"
        echo "$body" | jq -r '.message // .error // "Unknown error"' 2>/dev/null || echo "$body"
        return 1
    fi

    echo "$body"
}

# ========================================
# Authentication
# ========================================

horcrux_login() {
    local base_url="$1"
    local username="$2"
    local password="$3"

    HORCRUX_BASE_URL="$base_url"

    horcrux_log "Logging in to $base_url as $username..."

    local response
    response=$(curl -s -X POST \
        -H "Content-Type: application/json" \
        -d "{\"username\":\"$username\",\"password\":\"$password\"}" \
        "${base_url}/api/auth/login")

    HORCRUX_TOKEN=$(echo "$response" | jq -r '.token')

    if [ "$HORCRUX_TOKEN" = "null" ] || [ -z "$HORCRUX_TOKEN" ]; then
        horcrux_error "Login failed"
        return 1
    fi

    horcrux_log "Login successful"
}

horcrux_logout() {
    if [ -n "$HORCRUX_TOKEN" ]; then
        horcrux_api_call "POST" "/api/auth/logout" "" > /dev/null
        HORCRUX_TOKEN=""
        horcrux_log "Logged out"
    fi
}

# ========================================
# Virtual Machines
# ========================================

horcrux_list_vms() {
    horcrux_log "Listing VMs..."
    local response
    response=$(horcrux_api_call "GET" "/api/vms")

    if command -v jq &> /dev/null; then
        echo "$response" | jq -r '.[] | "\(.id)\t\(.name)\t\(.status)\t\(.cpus) CPUs\t\(.memory) MB"' | column -t
    else
        echo "$response"
    fi
}

horcrux_get_vm() {
    local vm_id="$1"
    horcrux_log "Getting VM $vm_id..."
    horcrux_api_call "GET" "/api/vms/$vm_id" | jq '.'
}

horcrux_create_vm() {
    local name="$1"
    local cpus="${2:-2}"
    local memory="${3:-2048}"
    local disk_size="${4:-20}"
    local hypervisor="${5:-Qemu}"

    horcrux_log "Creating VM $name..."

    local data="{\"name\":\"$name\",\"cpus\":$cpus,\"memory\":$memory,\"disk_size\":$disk_size,\"hypervisor\":\"$hypervisor\",\"architecture\":\"X86_64\"}"

    horcrux_api_call "POST" "/api/vms" "$data" | jq '.'
}

horcrux_start_vm() {
    local vm_id="$1"
    horcrux_log "Starting VM $vm_id..."
    horcrux_api_call "POST" "/api/vms/$vm_id/start" "" | jq '.'
}

horcrux_stop_vm() {
    local vm_id="$1"
    local force="${2:-false}"

    horcrux_log "Stopping VM $vm_id..."

    local data="{\"force\":$force}"
    horcrux_api_call "POST" "/api/vms/$vm_id/stop" "$data" | jq '.'
}

horcrux_delete_vm() {
    local vm_id="$1"
    horcrux_log "Deleting VM $vm_id..."
    horcrux_api_call "DELETE" "/api/vms/$vm_id" "" | jq '.'
}

# ========================================
# Snapshots
# ========================================

horcrux_list_snapshots() {
    local vm_id="$1"
    horcrux_log "Listing snapshots for VM $vm_id..."
    horcrux_api_call "GET" "/api/vms/$vm_id/snapshots" | jq '.'
}

horcrux_create_snapshot() {
    local vm_id="$1"
    local name="$2"
    local description="${3:-}"

    horcrux_log "Creating snapshot $name for VM $vm_id..."

    local data="{\"name\":\"$name\",\"description\":\"$description\"}"
    horcrux_api_call "POST" "/api/vms/$vm_id/snapshots" "$data" | jq '.'
}

horcrux_restore_snapshot() {
    local vm_id="$1"
    local snapshot_id="$2"

    horcrux_log "Restoring VM $vm_id to snapshot $snapshot_id..."
    horcrux_api_call "POST" "/api/vms/$vm_id/snapshots/$snapshot_id/restore" "" | jq '.'
}

horcrux_delete_snapshot() {
    local vm_id="$1"
    local snapshot_id="$2"

    horcrux_log "Deleting snapshot $snapshot_id from VM $vm_id..."
    horcrux_api_call "DELETE" "/api/vms/$vm_id/snapshots/$snapshot_id" "" | jq '.'
}

# ========================================
# Cloning
# ========================================

horcrux_clone_vm() {
    local vm_id="$1"
    local new_name="$2"
    local full_clone="${3:-true}"

    horcrux_log "Cloning VM $vm_id to $new_name..."

    local mode="full"
    if [ "$full_clone" = "false" ]; then
        mode="linked"
    fi

    local data="{\"name\":\"$new_name\",\"mode\":\"$mode\"}"
    horcrux_api_call "POST" "/api/vms/$vm_id/clone" "$data" | jq '.'
}

# ========================================
# Containers
# ========================================

horcrux_list_containers() {
    horcrux_log "Listing containers..."
    horcrux_api_call "GET" "/api/containers" | jq '.'
}

horcrux_create_container() {
    local name="$1"
    local runtime="${2:-Lxc}"
    local image="${3:-ubuntu:22.04}"
    local cpus="${4:-2}"
    local memory="${5:-2048}"

    horcrux_log "Creating container $name..."

    local data="{\"name\":\"$name\",\"runtime\":\"$runtime\",\"image\":\"$image\",\"cpus\":$cpus,\"memory\":$memory}"
    horcrux_api_call "POST" "/api/containers" "$data" | jq '.'
}

horcrux_start_container() {
    local container_id="$1"
    horcrux_log "Starting container $container_id..."
    horcrux_api_call "POST" "/api/containers/$container_id/start" "" | jq '.'
}

horcrux_stop_container() {
    local container_id="$1"
    horcrux_log "Stopping container $container_id..."
    horcrux_api_call "POST" "/api/containers/$container_id/stop" "" | jq '.'
}

horcrux_delete_container() {
    local container_id="$1"
    horcrux_log "Deleting container $container_id..."
    horcrux_api_call "DELETE" "/api/containers/$container_id" "" | jq '.'
}

# ========================================
# Backups
# ========================================

horcrux_list_backups() {
    local vm_id="${1:-}"
    horcrux_log "Listing backups..."

    if [ -n "$vm_id" ]; then
        horcrux_api_call "GET" "/api/backups?vm_id=$vm_id" | jq '.'
    else
        horcrux_api_call "GET" "/api/backups" | jq '.'
    fi
}

horcrux_create_backup() {
    local vm_id="$1"
    local backup_type="${2:-full}"
    local compression="${3:-zstd}"

    horcrux_log "Creating backup for VM $vm_id..."

    local data="{\"vm_id\":\"$vm_id\",\"type\":\"$backup_type\",\"compression\":\"$compression\"}"
    horcrux_api_call "POST" "/api/backups" "$data" | jq '.'
}

horcrux_restore_backup() {
    local backup_id="$1"
    local target_vm_id="${2:-}"

    horcrux_log "Restoring backup $backup_id..."

    local data="{}"
    if [ -n "$target_vm_id" ]; then
        data="{\"target_vm_id\":\"$target_vm_id\"}"
    fi

    horcrux_api_call "POST" "/api/backups/$backup_id/restore" "$data" | jq '.'
}

# ========================================
# Storage
# ========================================

horcrux_list_storage_pools() {
    horcrux_log "Listing storage pools..."
    horcrux_api_call "GET" "/api/storage/pools" | jq '.'
}

horcrux_get_storage_pool() {
    local pool_id="$1"
    horcrux_log "Getting storage pool $pool_id..."
    horcrux_api_call "GET" "/api/storage/pools/$pool_id" | jq '.'
}

# ========================================
# Clustering
# ========================================

horcrux_list_cluster_nodes() {
    horcrux_log "Listing cluster nodes..."
    horcrux_api_call "GET" "/api/cluster/nodes" | jq '.'
}

horcrux_get_cluster_architecture() {
    horcrux_log "Getting cluster architecture..."
    horcrux_api_call "GET" "/api/cluster/architecture" | jq '.'
}

# ========================================
# Monitoring
# ========================================

horcrux_get_node_stats() {
    horcrux_log "Getting node statistics..."
    horcrux_api_call "GET" "/api/monitoring/node" | jq '.'
}

horcrux_get_vm_stats() {
    local vm_id="$1"
    horcrux_log "Getting VM $vm_id statistics..."
    horcrux_api_call "GET" "/api/monitoring/vms/$vm_id" | jq '.'
}

horcrux_get_all_vm_stats() {
    horcrux_log "Getting all VM statistics..."
    horcrux_api_call "GET" "/api/monitoring/vms" | jq '.'
}

# ========================================
# Alerts
# ========================================

horcrux_list_alert_rules() {
    horcrux_log "Listing alert rules..."
    horcrux_api_call "GET" "/api/alerts/rules" | jq '.'
}

horcrux_list_active_alerts() {
    horcrux_log "Listing active alerts..."
    horcrux_api_call "GET" "/api/alerts/active" | jq '.'
}

# ========================================
# High Availability
# ========================================

horcrux_list_ha_resources() {
    horcrux_log "Listing HA resources..."
    horcrux_api_call "GET" "/api/ha/resources" | jq '.'
}

horcrux_get_ha_status() {
    horcrux_log "Getting HA status..."
    horcrux_api_call "GET" "/api/ha/status" | jq '.'
}

horcrux_add_ha_resource() {
    local vm_id="$1"
    local priority="${2:-100}"

    horcrux_log "Adding VM $vm_id to HA..."

    local data="{\"vm_id\":\"$vm_id\",\"priority\":$priority}"
    horcrux_api_call "POST" "/api/ha/resources" "$data" | jq '.'
}

# ========================================
# Firewall
# ========================================

horcrux_list_firewall_rules() {
    horcrux_log "Listing firewall rules..."
    horcrux_api_call "GET" "/api/firewall/rules" | jq '.'
}

horcrux_add_firewall_rule() {
    local name="$1"
    local action="$2"
    local protocol="$3"
    local port="$4"

    horcrux_log "Adding firewall rule $name..."

    local data="{\"name\":\"$name\",\"action\":\"$action\",\"protocol\":\"$protocol\",\"port\":$port,\"source\":\"0.0.0.0/0\",\"enabled\":true}"
    horcrux_api_call "POST" "/api/firewall/rules" "$data" | jq '.'
}

horcrux_apply_firewall_rules() {
    local scope="${1:-datacenter}"
    horcrux_log "Applying firewall rules for $scope..."
    horcrux_api_call "POST" "/api/firewall/$scope/apply" "" | jq '.'
}

# ========================================
# GPU
# ========================================

horcrux_list_gpu_devices() {
    horcrux_log "Listing GPU devices..."
    horcrux_api_call "GET" "/api/gpu/devices" | jq '.'
}

horcrux_check_iommu_status() {
    horcrux_log "Checking IOMMU status..."
    horcrux_api_call "GET" "/api/gpu/iommu-status" | jq '.'
}

# ========================================
# Utility Functions
# ========================================

horcrux_health() {
    local url="${HORCRUX_BASE_URL:-http://localhost:8006}"
    horcrux_log "Checking health..."
    curl -s "${url}/api/health" | jq '.'
}

horcrux_help() {
    cat << EOF
Horcrux Shell Client

Usage: source horcrux.sh

Authentication:
  horcrux_login <url> <username> <password>    Login to Horcrux
  horcrux_logout                               Logout

Virtual Machines:
  horcrux_list_vms                             List all VMs
  horcrux_get_vm <vm_id>                       Get VM details
  horcrux_create_vm <name> [cpus] [mem] [disk] Create VM
  horcrux_start_vm <vm_id>                     Start VM
  horcrux_stop_vm <vm_id> [force]              Stop VM
  horcrux_delete_vm <vm_id>                    Delete VM

Snapshots:
  horcrux_list_snapshots <vm_id>               List VM snapshots
  horcrux_create_snapshot <vm_id> <name>       Create snapshot
  horcrux_restore_snapshot <vm_id> <snap_id>   Restore snapshot
  horcrux_delete_snapshot <vm_id> <snap_id>    Delete snapshot

Cloning:
  horcrux_clone_vm <vm_id> <new_name> [full]   Clone VM

Containers:
  horcrux_list_containers                      List containers
  horcrux_create_container <name> [runtime]    Create container
  horcrux_start_container <id>                 Start container
  horcrux_stop_container <id>                  Stop container
  horcrux_delete_container <id>                Delete container

Backups:
  horcrux_list_backups [vm_id]                 List backups
  horcrux_create_backup <vm_id> [type]         Create backup
  horcrux_restore_backup <backup_id> [vm_id]   Restore backup

Storage:
  horcrux_list_storage_pools                   List storage pools
  horcrux_get_storage_pool <pool_id>           Get pool details

Clustering:
  horcrux_list_cluster_nodes                   List cluster nodes
  horcrux_get_cluster_architecture             Get cluster architecture

Monitoring:
  horcrux_get_node_stats                       Get node statistics
  horcrux_get_vm_stats <vm_id>                 Get VM statistics
  horcrux_get_all_vm_stats                     Get all VM statistics

Alerts:
  horcrux_list_alert_rules                     List alert rules
  horcrux_list_active_alerts                   List active alerts

High Availability:
  horcrux_list_ha_resources                    List HA resources
  horcrux_get_ha_status                        Get HA status
  horcrux_add_ha_resource <vm_id> [priority]   Add VM to HA

Firewall:
  horcrux_list_firewall_rules                  List firewall rules
  horcrux_add_firewall_rule <name> <action> <proto> <port>
  horcrux_apply_firewall_rules [scope]         Apply firewall rules

GPU:
  horcrux_list_gpu_devices                     List GPU devices
  horcrux_check_iommu_status                   Check IOMMU status

Utility:
  horcrux_health                               Check API health
  horcrux_help                                 Show this help

EOF
}

# Check dependencies
horcrux_check_deps

# Show help if script is executed (not sourced)
if [ "${BASH_SOURCE[0]}" == "${0}" ]; then
    horcrux_help
fi
