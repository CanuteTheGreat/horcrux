#!/bin/bash
# Automated Backup Configuration Example
#
# This script sets up automated backup schedules for Horcrux VMs

set -e

# Daily incremental backups at 2 AM
echo "Creating daily incremental backup schedule..."
horcrux-cli backup create-schedule \
  --name "daily-incremental" \
  --vms "vm-*" \
  --type incremental \
  --schedule "0 2 * * *" \
  --retention 7 \
  --compression zstd \
  --destination /backup/horcrux/daily

# Weekly full backups on Sunday at 1 AM
echo "Creating weekly full backup schedule..."
horcrux-cli backup create-schedule \
  --name "weekly-full" \
  --vms "vm-*" \
  --type full \
  --schedule "0 1 * * 0" \
  --retention 4 \
  --compression zstd \
  --destination /backup/horcrux/weekly

# Critical VMs - more frequent backups
echo "Creating critical VM backup schedule..."
horcrux-cli backup create-schedule \
  --name "critical-hourly" \
  --vms "critical-*" \
  --type incremental \
  --schedule "0 * * * *" \
  --retention 24 \
  --compression lz4 \
  --destination /backup/horcrux/critical

# Off-site backup to S3 (monthly)
echo "Creating monthly S3 backup schedule..."
horcrux-cli backup create-schedule \
  --name "monthly-offsite" \
  --vms "vm-*" \
  --type full \
  --schedule "0 3 1 * *" \
  --retention 12 \
  --compression zstd \
  --destination s3://my-bucket/horcrux-backups

echo "Backup schedules created successfully!"
echo ""
echo "To list all schedules:"
echo "  horcrux-cli backup list-schedules"
echo ""
echo "To manually trigger a backup:"
echo "  horcrux-cli backup create --vm <vm-name> --type full"
