#!/bin/bash
# High Availability Setup Example
#
# This script configures HA for critical VMs using Pacemaker

set -e

echo "=== Horcrux High Availability Setup ==="
echo ""

# Create HA group for critical services
echo "1. Creating HA group for critical services..."
horcrux-cli ha create-group \
  --name critical-services \
  --nodes node1,node2,node3 \
  --nofailback false \
  --priority 100

# Add VMs to HA management
echo "2. Adding VMs to HA management..."

# Database server - highest priority
horcrux-cli ha add-vm \
  --vm-id db-server \
  --group critical-services \
  --priority 200 \
  --max-restart 3 \
  --preferred-node node1 \
  --state started

# Web server - medium priority
horcrux-cli ha add-vm \
  --vm-id web-server \
  --group critical-services \
  --priority 100 \
  --max-restart 3 \
  --preferred-node node2 \
  --state started

# Application server - medium priority
horcrux-cli ha add-vm \
  --vm-id app-server \
  --group critical-services \
  --priority 100 \
  --max-restart 3 \
  --preferred-node node3 \
  --state started

# Configure HA policies
echo "3. Configuring HA policies..."

# Set migration timeout
horcrux-cli ha set-policy \
  --name migration-timeout \
  --value 120

# Set restart delay
horcrux-cli ha set-policy \
  --name restart-delay \
  --value 30

# Enable auto-recovery
horcrux-cli ha set-policy \
  --name auto-recovery \
  --value true

echo ""
echo "=== HA Configuration Complete ==="
echo ""
echo "To check HA status:"
echo "  horcrux-cli ha status"
echo ""
echo "To manually failover a VM:"
echo "  horcrux-cli ha migrate <vm-id> --target-node <node>"
echo ""
echo "To test failover:"
echo "  # Simulate node failure by stopping Pacemaker on preferred node"
echo "  ssh node1 'sudo systemctl stop pacemaker'"
echo "  # VM should automatically migrate to another node"
echo "  horcrux-cli ha status"
echo "  # Restore node"
echo "  ssh node1 'sudo systemctl start pacemaker'"
