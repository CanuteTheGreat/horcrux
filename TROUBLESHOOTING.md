# Horcrux Troubleshooting Guide

Common issues and solutions for Horcrux deployment and operation.

## Table of Contents

- [Installation Issues](#installation-issues)
- [API Server Issues](#api-server-issues)
- [VM Issues](#vm-issues)
- [Storage Issues](#storage-issues)
- [Cluster Issues](#cluster-issues)
- [Network Issues](#network-issues)
- [Performance Issues](#performance-issues)
- [Authentication Issues](#authentication-issues)
- [Debugging Tools](#debugging-tools)

## Installation Issues

### Build Fails with Dependency Errors

**Symptom:** `cargo build` fails with dependency resolution errors

**Solution:**
```bash
# Update Rust to latest stable
rustup update stable

# Clean build cache
cargo clean

# Rebuild
cargo build --release
```

### Missing System Dependencies

**Symptom:** Build fails with "linker error" or "library not found"

**Solution (Gentoo):**
```bash
sudo emerge -av openssl sqlite
```

**Solution (Ubuntu/Debian):**
```bash
sudo apt install build-essential libssl-dev libsqlite3-dev pkg-config
```

### WASM Target Not Found

**Symptom:** UI build fails with "target not found"

**Solution:**
```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
```

## API Server Issues

### Server Won't Start - Port in Use

**Symptom:**
```
Error: Address already in use (os error 98)
```

**Diagnosis:**
```bash
# Check what's using port 8006
sudo lsof -i :8006
sudo ss -tlnp | grep 8006
```

**Solution:**
```bash
# Option 1: Stop conflicting service
sudo systemctl stop <conflicting-service>

# Option 2: Change port in config
# Edit /etc/horcrux/config.toml
[server]
bind_address = "0.0.0.0:8007"
```

### Server Won't Start - Permission Denied

**Symptom:**
```
Error: Permission denied (os error 13)
```

**Diagnosis:**
```bash
# Check database permissions
ls -la /var/lib/horcrux/

# Check log directory permissions
ls -la /var/log/horcrux/
```

**Solution:**
```bash
# Fix ownership
sudo chown -R horcrux:horcrux /var/lib/horcrux/
sudo chown -R horcrux:horcrux /var/log/horcrux/

# Fix permissions
sudo chmod 755 /var/lib/horcrux/
sudo chmod 644 /var/lib/horcrux/horcrux.db
```

### Database Locked Error

**Symptom:**
```
Error: database is locked
```

**Solution:**
```bash
# Check for stale processes
ps aux | grep horcrux-api

# Kill stale processes
sudo pkill -9 horcrux-api

# Remove lock file (if exists)
sudo rm -f /var/lib/horcrux/horcrux.db-shm
sudo rm -f /var/lib/horcrux/horcrux.db-wal

# Restart service
sudo systemctl restart horcrux
```

### High CPU Usage

**Symptom:** API server consuming excessive CPU

**Diagnosis:**
```bash
# Check API server performance
top -p $(pgrep horcrux-api)

# Check logs for errors
sudo journalctl -u horcrux -n 100

# Check for runaway VMs
horcrux-cli vm list
```

**Solution:**
```bash
# Reduce worker threads
# Edit /etc/horcrux/config.toml
[server]
workers = 4  # Reduce from default

# Enable request rate limiting
[server]
rate_limit_enabled = true
rate_limit_requests = 100  # per minute
```

## VM Issues

### VM Won't Start - QEMU Not Found

**Symptom:**
```
Error: QEMU not found
```

**Solution:**
```bash
# Install QEMU
sudo emerge qemu                    # Gentoo
sudo apt install qemu-system-x86    # Ubuntu/Debian

# Verify installation
which qemu-system-x86_64
qemu-system-x86_64 --version
```

### VM Won't Start - KVM Permission Denied

**Symptom:**
```
Error: Could not access KVM kernel module: Permission denied
```

**Solution:**
```bash
# Add user to kvm group
sudo usermod -aG kvm $USER

# Load KVM module
sudo modprobe kvm
sudo modprobe kvm_intel  # or kvm_amd for AMD

# Verify
ls -la /dev/kvm
# Should show: crw-rw---- 1 root kvm

# Re-login or use newgrp
newgrp kvm
```

### VM Won't Start - Insufficient Memory

**Symptom:**
```
Error: Cannot allocate memory
```

**Diagnosis:**
```bash
# Check available memory
free -h

# Check VM configuration
horcrux-cli vm status <vm-name>

# Check for memory overcommit
sysctl vm.overcommit_memory
```

**Solution:**
```bash
# Option 1: Reduce VM memory
horcrux-cli vm update <vm-name> --memory 2048

# Option 2: Enable memory overcommit (use with caution)
sudo sysctl -w vm.overcommit_memory=1

# Option 3: Stop other VMs
horcrux-cli vm stop <other-vm>
```

### VM Crashes Immediately

**Symptom:** VM starts but crashes within seconds

**Diagnosis:**
```bash
# Check VM logs
sudo tail -f /var/log/horcrux/vm-<id>.log

# Check QEMU logs
sudo journalctl -u horcrux | grep "<vm-id>"

# Try starting VM manually for debugging
qemu-system-x86_64 \
  -m 2048 \
  -smp 2 \
  -hda /var/lib/horcrux/vms/<vm-disk>.qcow2 \
  -nographic
```

**Common Causes:**
1. Corrupted disk image
2. Incompatible CPU features
3. Missing bootloader
4. Insufficient resources

**Solution:**
```bash
# Check disk image
qemu-img check /var/lib/horcrux/vms/<vm-disk>.qcow2

# Repair if needed
qemu-img check -r all /var/lib/horcrux/vms/<vm-disk>.qcow2

# Use compatible CPU model
horcrux-cli vm update <vm-name> --cpu-model qemu64
```

### VNC Console Not Working

**Symptom:** Cannot connect to VM console

**Diagnosis:**
```bash
# Check VNC port
horcrux-cli vm status <vm-name> | grep vnc_port

# Check if port is listening
sudo netstat -tlnp | grep <vnc-port>

# Check firewall
sudo iptables -L -n | grep <vnc-port>
```

**Solution:**
```bash
# Allow VNC through firewall
sudo iptables -A INPUT -p tcp --dport 5900:5999 -j ACCEPT
sudo iptables-save > /etc/iptables/rules.v4

# Restart VM
horcrux-cli vm stop <vm-name>
horcrux-cli vm start <vm-name>
```

## Storage Issues

### Storage Pool Creation Fails

**Symptom:**
```
Error: Storage pool validation failed
```

**Diagnosis by Storage Type:**

**ZFS:**
```bash
# Check if pool exists
sudo zpool list
sudo zpool status tank

# Check if ZFS is loaded
lsmod | grep zfs
```

**Ceph:**
```bash
# Check Ceph status
sudo ceph status
sudo ceph osd pool ls

# Check Ceph configuration
cat /etc/ceph/ceph.conf
```

**NFS:**
```bash
# Check if NFS server is reachable
showmount -e <nfs-server>
ping <nfs-server>

# Try manual mount
sudo mount -t nfs <nfs-server>:/path /mnt/test
```

### Disk Full Error

**Symptom:**
```
Error: No space left on device
```

**Diagnosis:**
```bash
# Check disk usage
df -h

# Check inode usage
df -i

# Find large files
sudo du -h /var/lib/horcrux | sort -rh | head -20

# Check for deleted but open files
sudo lsof | grep deleted
```

**Solution:**
```bash
# Clean up old backups
horcrux-cli backup list --old
horcrux-cli backup delete <backup-id>

# Clean up VM snapshots
horcrux-cli vm snapshot list <vm-name>
horcrux-cli vm snapshot delete <vm-name> <snapshot>

# Increase disk space or add storage pool
```

### Slow Disk Performance

**Symptom:** VMs have poor disk I/O performance

**Diagnosis:**
```bash
# Test disk performance
sudo hdparm -Tt /dev/sda

# Check I/O wait
iostat -x 5

# Check for disk errors
sudo dmesg | grep -i error
```

**Solution:**

**For ZFS:**
```bash
# Enable compression
sudo zfs set compression=lz4 tank/horcrux

# Adjust recordsize
sudo zfs set recordsize=64K tank/horcrux

# Add L2ARC cache
sudo zpool add tank cache /dev/sdb
```

**For qcow2:**
```bash
# Convert to raw format (better performance)
qemu-img convert -f qcow2 -O raw \
  /path/to/vm.qcow2 \
  /path/to/vm.raw

# Use preallocated qcow2
qemu-img create -f qcow2 -o preallocation=metadata \
  /path/to/vm.qcow2 50G
```

## Cluster Issues

### Node Can't Join Cluster

**Symptom:**
```
Error: Failed to join cluster
```

**Diagnosis:**
```bash
# Check network connectivity
ping <primary-node-ip>

# Check cluster ports
sudo nmap -p 5404-5406 <primary-node-ip>

# Check Corosync status
sudo corosync-cfgtool -s

# Check firewall
sudo iptables -L -n | grep 5404
```

**Solution:**
```bash
# Open cluster ports
sudo iptables -A INPUT -p udp --dport 5404:5406 -j ACCEPT
sudo iptables -A INPUT -p tcp --dport 5404:5406 -j ACCEPT

# Restart Corosync
sudo systemctl restart corosync

# Verify multicast (if using)
sudo corosync-cfgtool -s
```

### Split-Brain Scenario

**Symptom:** Cluster has conflicting state, multiple nodes think they're primary

**Diagnosis:**
```bash
# Check quorum on each node
sudo crm status

# Check cluster membership
sudo pcs status

# Check Corosync logs
sudo journalctl -u corosync -n 100
```

**Solution:**
```bash
# DO NOT attempt automatic recovery
# Manual intervention required

# 1. Identify the most up-to-date node
# Check timestamps of VM states on each node

# 2. Stop Pacemaker on all nodes
sudo systemctl stop pacemaker

# 3. Stop Corosync on all nodes except the correct primary
sudo systemctl stop corosync

# 4. Clear incorrect state on secondary nodes
sudo rm -rf /var/lib/pacemaker/cib/*

# 5. Restart cluster starting with primary
# On primary:
sudo systemctl start corosync
sudo systemctl start pacemaker

# On secondaries (one at a time):
sudo systemctl start corosync
sudo systemctl start pacemaker
```

### Quorum Loss

**Symptom:**
```
Error: Cluster does not have quorum
```

**Solution:**
```bash
# Check cluster status
sudo pcs status

# Option 1: Bring up missing nodes
sudo systemctl start corosync  # on each missing node

# Option 2: Force quorum (EMERGENCY ONLY)
sudo pcs quorum unblock

# Option 3: Adjust quorum settings (2-node cluster)
sudo pcs property set no-quorum-policy=ignore
```

## Network Issues

### VMs Can't Access Network

**Symptom:** VMs have no network connectivity

**Diagnosis:**
```bash
# Check bridge status
sudo ip link show br0

# Check if bridge is up
sudo brctl show

# Check VM network interface
horcrux-cli vm status <vm-name> | grep network

# Check if traffic is flowing
sudo tcpdump -i br0
```

**Solution:**
```bash
# Create bridge if missing
sudo ip link add br0 type bridge
sudo ip link set br0 up
sudo ip link set eth1 master br0

# Enable IP forwarding
sudo sysctl -w net.ipv4.ip_forward=1
echo "net.ipv4.ip_forward=1" | sudo tee -a /etc/sysctl.conf

# Check iptables rules
sudo iptables -L -n -v
```

### Live Migration Fails

**Symptom:**
```
Error: Migration failed
```

**Diagnosis:**
```bash
# Check network connectivity between nodes
ping <target-node>

# Check SSH connectivity
ssh <target-node> echo "OK"

# Check migration ports
sudo nmap -p 60000-60050 <target-node>

# Check available resources on target
ssh <target-node> "free -h && df -h"
```

**Solution:**
```bash
# Ensure migration ports are open
sudo iptables -A INPUT -p tcp --dport 60000:60050 -j ACCEPT

# Ensure shared storage is accessible
df -h | grep <storage-mount>

# Check VM is using shared storage
horcrux-cli vm status <vm-name>
```

## Performance Issues

### Slow API Response Times

**Symptom:** API requests take several seconds

**Diagnosis:**
```bash
# Check API server load
curl -w "@-" -o /dev/null -s http://localhost:8006/api/health <<'EOF'
    time_namelookup:  %{time_namelookup}\n
       time_connect:  %{time_connect}\n
    time_appconnect:  %{time_appconnect}\n
   time_pretransfer:  %{time_pretransfer}\n
      time_redirect:  %{time_redirect}\n
 time_starttransfer:  %{time_starttransfer}\n
                    ----------\n
         time_total:  %{time_total}\n
EOF

# Check database performance
sqlite3 /var/lib/horcrux/horcrux.db "PRAGMA integrity_check;"

# Check system load
top
```

**Solution:**
```bash
# Optimize database
sqlite3 /var/lib/horcrux/horcrux.db "VACUUM; ANALYZE;"

# Increase worker threads
# Edit /etc/horcrux/config.toml
[server]
workers = 16  # Match CPU core count

# Increase database pool
[database]
pool_size = 20
```

### High Memory Usage

**Symptom:** Horcrux API server using excessive memory

**Diagnosis:**
```bash
# Check memory usage
ps aux | grep horcrux-api

# Check for memory leaks
valgrind --leak-check=full ./target/release/horcrux-api
```

**Solution:**
```bash
# Restart service periodically
sudo systemctl restart horcrux

# Reduce caching
# Edit /etc/horcrux/config.toml
[server]
cache_size = 100  # Reduce from default
```

## Authentication Issues

### Login Fails

**Symptom:**
```
Error: Authentication failed
```

**Diagnosis:**
```bash
# Check if user exists
horcrux-cli user list

# Check auth logs
sudo journalctl -u horcrux | grep auth

# Test authentication manually
curl -X POST http://localhost:8006/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin"}'
```

**Solution:**
```bash
# Reset user password
horcrux-cli user password-reset admin

# Check authentication configuration
cat /etc/horcrux/config.toml | grep -A 10 "\[auth\]"
```

### Session Expires Too Quickly

**Symptom:** Need to login frequently

**Solution:**
```bash
# Increase session timeout
# Edit /etc/horcrux/config.toml
[auth]
session_timeout = 14400  # 4 hours (in seconds)

# Restart service
sudo systemctl restart horcrux
```

## Debugging Tools

### Enable Debug Logging

```bash
# Method 1: Environment variable
export RUST_LOG=debug
sudo systemctl restart horcrux

# Method 2: Configuration file
# Edit /etc/horcrux/config.toml
[logging]
level = "debug"
```

### Collect Diagnostic Information

```bash
# Create diagnostic report
cat > /tmp/horcrux-diag.sh << 'EOF'
#!/bin/bash
echo "=== Horcrux Diagnostic Report ===" > /tmp/horcrux-diag.txt
echo "Generated: $(date)" >> /tmp/horcrux-diag.txt
echo "" >> /tmp/horcrux-diag.txt

echo "=== System Info ===" >> /tmp/horcrux-diag.txt
uname -a >> /tmp/horcrux-diag.txt
free -h >> /tmp/horcrux-diag.txt
df -h >> /tmp/horcrux-diag.txt

echo "=== Horcrux Version ===" >> /tmp/horcrux-diag.txt
horcrux-api --version >> /tmp/horcrux-diag.txt 2>&1

echo "=== Service Status ===" >> /tmp/horcrux-diag.txt
systemctl status horcrux >> /tmp/horcrux-diag.txt 2>&1

echo "=== Recent Logs ===" >> /tmp/horcrux-diag.txt
journalctl -u horcrux -n 50 >> /tmp/horcrux-diag.txt 2>&1

echo "=== Configuration ===" >> /tmp/horcrux-diag.txt
cat /etc/horcrux/config.toml >> /tmp/horcrux-diag.txt 2>&1

echo "=== Network ===" >> /tmp/horcrux-diag.txt
ip addr >> /tmp/horcrux-diag.txt
ss -tlnp | grep 8006 >> /tmp/horcrux-diag.txt

echo "Report saved to /tmp/horcrux-diag.txt"
EOF

chmod +x /tmp/horcrux-diag.sh
/tmp/horcrux-diag.sh
```

### Test API Connectivity

```bash
# Health check
curl -v http://localhost:8006/api/health

# List VMs
curl http://localhost:8006/api/vms

# Check with authentication
TOKEN=$(curl -s -X POST http://localhost:8006/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin"}' \
  | jq -r '.token')

curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:8006/api/vms
```

## Getting Help

If your issue isn't covered here:

1. **Check logs:** `sudo journalctl -u horcrux -f`
2. **Enable debug logging:** See [Enable Debug Logging](#enable-debug-logging)
3. **Search issues:** [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
4. **Create diagnostic report:** See [Collect Diagnostic Information](#collect-diagnostic-information)
5. **Open an issue:** Include diagnostic report and steps to reproduce

## Additional Resources

- [Quick Start Guide](QUICKSTART.md)
- [Deployment Guide](DEPLOYMENT.md)
- [API Documentation](docs/API.md)
- [Contributing Guide](CONTRIBUTING.md)

---

**Still stuck?** Open an issue with your diagnostic report: [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
