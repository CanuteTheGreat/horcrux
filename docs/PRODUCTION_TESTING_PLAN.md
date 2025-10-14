# Production Integration Testing Plan

**Version**: 1.0
**Date**: 2025-10-13
**Target Release**: v0.2.0
**Status**: Draft

---

## Overview

This document outlines the comprehensive integration testing plan for Horcrux on production hardware. WSL2 testing verified protocol correctness; production testing will validate real-world functionality with actual VMs.

**Testing Goals**:
1. Verify all features work with real QEMU/KVM VMs
2. Validate performance under load
3. Ensure security measures are effective
4. Confirm multi-user/multi-VM scenarios
5. Test disaster recovery and failover

---

## Testing Environment Requirements

### Hardware

**Minimum Specifications**:
```
CPU: Intel/AMD with VT-x/AMD-V (virtualization extensions)
RAM: 16 GB minimum (32 GB recommended)
Storage: 100 GB SSD
Network: 1 Gbps ethernet
```

**Recommended Specifications**:
```
CPU: Intel Xeon or AMD EPYC (8+ cores)
RAM: 64 GB ECC RAM
Storage: 500 GB NVMe SSD (or multiple drives)
Network: 10 Gbps with redundancy
GPU: Optional (for vGPU testing)
```

### Software

**Operating System**:
- Ubuntu 22.04/24.04 LTS
- Debian 12
- RHEL 9 / Rocky Linux 9
- **NOT WSL2** (requires native Linux with KVM)

**Required Packages**:
```bash
# Ubuntu/Debian
sudo apt-get install qemu-kvm libvirt-daemon-system libvirt-clients \
                     bridge-utils virt-manager ovmf \
                     postgresql sqlite3 docker.io

# RHEL/Rocky
sudo dnf install qemu-kvm libvirt virt-install bridge-utils \
                 postgresql-server sqlite docker
```

**Verification**:
```bash
# Check KVM support
lsmod | grep kvm
kvm-ok  # Ubuntu only

# Check libvirt
virsh version
systemctl status libvirtd

# Check Docker
docker --version
systemctl status docker
```

---

## Test Phases

### Phase 1: Basic Functionality (Week 1)
**Duration**: 3-5 days
**Goal**: Verify core features work

### Phase 2: Integration Testing (Week 1-2)
**Duration**: 5-7 days
**Goal**: Test component interaction

### Phase 3: Performance Testing (Week 2)
**Duration**: 3-5 days
**Goal**: Measure performance under load

### Phase 4: Security Testing (Week 2-3)
**Duration**: 3-5 days
**Goal**: Validate security measures

### Phase 5: Stress Testing (Week 3)
**Duration**: 2-3 days
**Goal**: Find breaking points

### Phase 6: User Acceptance Testing (Week 3-4)
**Duration**: 5-7 days
**Goal**: Real-world usage validation

---

## Phase 1: Basic Functionality Testing

### 1.1 VM Lifecycle Management

**Test Cases**:
- [ ] Create VM with various configurations
  - [ ] x86_64 architecture
  - [ ] aarch64 architecture (if ARM hardware available)
  - [ ] 1 CPU, 512 MB RAM (minimal)
  - [ ] 8 CPU, 16 GB RAM (typical)
  - [ ] 32 CPU, 64 GB RAM (maximum)
- [ ] Start VM
  - [ ] Verify VM boots successfully
  - [ ] Check VNC/console access
  - [ ] Verify network connectivity
- [ ] Stop VM (graceful shutdown)
  - [ ] Verify clean shutdown (no corruption)
  - [ ] Check disk integrity
- [ ] Force stop VM (kill)
  - [ ] Verify immediate termination
  - [ ] Check recovery mechanism
- [ ] Pause/Resume VM
  - [ ] Verify state preservation
  - [ ] Check memory consistency
- [ ] Delete VM
  - [ ] Verify complete cleanup
  - [ ] Check disk space reclamation

**Success Criteria**:
- All VM operations complete within expected time
- No data corruption or loss
- Proper error handling and logging

---

### 1.2 Console Access (noVNC)

**Test Cases**:
- [ ] Generate console ticket
  - [ ] Verify ticket creation
  - [ ] Check ticket expiration (5 min)
  - [ ] Confirm UUID format
- [ ] Access noVNC HTML page
  - [ ] Load in Chrome
  - [ ] Load in Firefox
  - [ ] Load in Safari
  - [ ] Load in Edge
- [ ] VNC connection
  - [ ] Establish WebSocket connection
  - [ ] Verify framebuffer display
  - [ ] Check screen refresh rate
- [ ] Input handling
  - [ ] Keyboard input
  - [ ] Mouse clicks
  - [ ] Mouse movement
  - [ ] Special keys (Ctrl, Alt, etc.)
- [ ] Clipboard sync
  - [ ] Copy from client to VM
  - [ ] Copy from VM to client
- [ ] Reconnection
  - [ ] Disconnect and reconnect
  - [ ] Verify session persistence
- [ ] Multi-user access
  - [ ] Multiple clients to same VM
  - [ ] Verify shared/exclusive modes

**Success Criteria**:
- Console loads within 2 seconds
- Keyboard/mouse input lag < 50ms
- Screen updates smooth (>= 15 FPS)
- Clipboard sync works bidirectionally

---

### 1.3 Storage Management

**Test Cases**:
- [ ] Create storage pool
  - [ ] Directory-based pool
  - [ ] LVM pool
  - [ ] ZFS pool (if available)
  - [ ] Ceph pool (if available)
- [ ] Create disk image
  - [ ] qcow2 format
  - [ ] raw format
  - [ ] Thin provisioning
  - [ ] Pre-allocation
- [ ] Attach disk to VM
  - [ ] IDE bus
  - [ ] SCSI bus
  - [ ] VirtIO bus
- [ ] Detach disk from VM
  - [ ] Hot-plug test
  - [ ] Cold-plug test
- [ ] Resize disk
  - [ ] Expand (online)
  - [ ] Expand (offline)
- [ ] Snapshot disk
  - [ ] Internal snapshot
  - [ ] External snapshot
  - [ ] Snapshot chain
- [ ] Delete storage pool
  - [ ] Verify cleanup
  - [ ] Check space reclamation

**Success Criteria**:
- All storage operations succeed
- No data loss or corruption
- Proper quota enforcement

---

### 1.4 Network Management

**Test Cases**:
- [ ] Create network
  - [ ] NAT network
  - [ ] Bridge network
  - [ ] Isolated network
  - [ ] VLAN network
- [ ] Attach VM to network
  - [ ] Single NIC
  - [ ] Multiple NICs
  - [ ] NIC hot-plug
- [ ] Network connectivity
  - [ ] VM to host
  - [ ] VM to internet
  - [ ] VM to VM (same network)
  - [ ] VM to VM (different network)
- [ ] Bandwidth limiting
  - [ ] Ingress limit
  - [ ] Egress limit
  - [ ] Burst allowance
- [ ] Firewall rules
  - [ ] Allow specific ports
  - [ ] Block specific IPs
  - [ ] Rate limiting
- [ ] Delete network
  - [ ] Verify cleanup
  - [ ] Check port release

**Success Criteria**:
- Network operations complete quickly
- Connectivity as expected
- Firewall rules enforced correctly

---

## Phase 2: Integration Testing

### 2.1 Docker Integration

**Test Cases**:
- [ ] List Docker containers
  - [ ] Verify all containers detected
  - [ ] Check status accuracy
- [ ] Get container stats
  - [ ] CPU usage
  - [ ] Memory usage
  - [ ] Network I/O
  - [ ] Block I/O
- [ ] Container lifecycle
  - [ ] Start container
  - [ ] Stop container
  - [ ] Restart container
  - [ ] Delete container
- [ ] Multi-container apps
  - [ ] Docker Compose stack
  - [ ] Service discovery
  - [ ] Load balancing

**Success Criteria**:
- All Docker API calls succeed
- Stats accuracy within 5%
- No container state desync

---

### 2.2 Metrics Collection

**Test Cases**:
- [ ] System metrics
  - [ ] CPU usage (per-core)
  - [ ] Memory usage
  - [ ] Disk I/O
  - [ ] Network I/O
  - [ ] Load average
  - [ ] Uptime
- [ ] VM metrics (libvirt)
  - [ ] CPU time
  - [ ] Memory usage
  - [ ] Disk read/write
  - [ ] Network RX/TX
- [ ] Container metrics
  - [ ] Docker API stats
  - [ ] cgroups v1 fallback
  - [ ] cgroups v2 support
- [ ] Metrics export
  - [ ] Prometheus format
  - [ ] JSON format
  - [ ] CSV export

**Success Criteria**:
- Metrics collected every 5 seconds
- <1% CPU overhead
- <100 MB memory overhead
- No metric gaps or errors

---

### 2.3 Authentication & Authorization

**Test Cases**:
- [ ] User management
  - [ ] Create user
  - [ ] Login with password
  - [ ] Login with API token
  - [ ] Change password
  - [ ] Delete user
- [ ] Session management
  - [ ] Create session
  - [ ] Validate session
  - [ ] Extend session
  - [ ] Expire session
- [ ] RBAC (if implemented)
  - [ ] Admin role
  - [ ] User role
  - [ ] Read-only role
  - [ ] Permission checks
- [ ] PAM integration (if implemented)
  - [ ] System user login
  - [ ] AD/LDAP integration
- [ ] API token management
  - [ ] Generate token
  - [ ] Validate token
  - [ ] Revoke token
  - [ ] Token expiration

**Success Criteria**:
- All auth methods work
- Session timeout enforced
- RBAC permissions correct
- No authentication bypass

---

### 2.4 Backup & Restore

**Test Cases**:
- [ ] Backup VM
  - [ ] Full backup (qcow2)
  - [ ] Incremental backup
  - [ ] Scheduled backup
- [ ] Backup storage
  - [ ] Local directory
  - [ ] NFS share
  - [ ] S3-compatible storage
- [ ] Restore VM
  - [ ] Full restore
  - [ ] Selective file restore
  - [ ] Restore to different name
- [ ] Backup verification
  - [ ] Checksum validation
  - [ ] Integrity check
  - [ ] Test restore

**Success Criteria**:
- Backup completes successfully
- Restore matches original
- No data corruption
- Scheduled backups run automatically

---

## Phase 3: Performance Testing

### 3.1 Load Testing

**Test Scenarios**:
- [ ] 10 concurrent VMs
  - [ ] All starting simultaneously
  - [ ] Mixed workloads
  - [ ] Measure start time
- [ ] 25 concurrent VMs
  - [ ] Sustained operation
  - [ ] CPU/memory usage
  - [ ] Network throughput
- [ ] 50 concurrent VMs (if hardware supports)
  - [ ] System stability
  - [ ] Resource exhaustion handling
- [ ] 100 concurrent noVNC connections
  - [ ] WebSocket proxy load
  - [ ] Latency measurement
  - [ ] Connection stability

**Metrics to Collect**:
```
VM Start Time: Target < 30s
API Response Time: Target < 100ms
noVNC Latency: Target < 50ms
CPU Usage: Target < 80%
Memory Usage: Target < 90%
Disk I/O Wait: Target < 5%
```

**Success Criteria**:
- System remains stable under load
- All metrics within targets
- No crashes or hangs
- Graceful degradation when overloaded

---

### 3.2 Stress Testing

**Test Scenarios**:
- [ ] Memory pressure
  - [ ] Overcommit memory
  - [ ] Monitor OOM killer
  - [ ] Verify swapping behavior
- [ ] CPU saturation
  - [ ] 100% CPU load
  - [ ] Context switch overhead
  - [ ] Scheduler fairness
- [ ] Disk I/O stress
  - [ ] Heavy write load
  - [ ] Heavy read load
  - [ ] Mixed workload
  - [ ] Check I/O scheduler
- [ ] Network saturation
  - [ ] High packet rate
  - [ ] Large transfers
  - [ ] Many connections

**Tools to Use**:
```bash
# CPU stress
stress-ng --cpu 8 --timeout 60s

# Memory stress
stress-ng --vm 4 --vm-bytes 8G --timeout 60s

# Disk I/O stress
fio --name=randwrite --ioengine=libaio --iodepth=32 --rw=randwrite \
    --bs=4k --direct=1 --size=1G --numjobs=4 --runtime=60

# Network stress
iperf3 -c <host> -P 10 -t 60
```

**Success Criteria**:
- System survives stress tests
- Services remain responsive
- No data corruption
- Recovery after stress ends

---

### 3.3 Benchmark Suite

**Benchmarks to Run**:
- [ ] VM I/O performance
  - [ ] Sequential read/write
  - [ ] Random read/write
  - [ ] IOPS measurement
- [ ] Network performance
  - [ ] Throughput (Gbps)
  - [ ] Latency (ms)
  - [ ] Packet loss (%)
- [ ] CPU performance
  - [ ] Single-core
  - [ ] Multi-core
  - [ ] Context switches
- [ ] Memory performance
  - [ ] Bandwidth (GB/s)
  - [ ] Latency (ns)

**Comparison Baseline**:
Compare against:
1. Native Linux performance
2. Raw QEMU (no Horcrux)
3. Other platforms (Proxmox, OpenStack)

**Success Criteria**:
- Performance within 10% of baseline
- No significant regressions
- Meets stated specifications

---

## Phase 4: Security Testing

### 4.1 Authentication Security

**Test Cases**:
- [ ] Password security
  - [ ] Weak password rejection
  - [ ] Password hashing (bcrypt/argon2)
  - [ ] Brute force protection
  - [ ] Rate limiting
- [ ] Session security
  - [ ] Session hijacking prevention
  - [ ] CSRF protection
  - [ ] XSS prevention
- [ ] API token security
  - [ ] Token entropy (sufficient randomness)
  - [ ] Token revocation
  - [ ] Scoped permissions

**Tools to Use**:
```bash
# Authentication testing
hydra -l admin -P passwords.txt http-post-form "/api/auth/login"

# Session testing
burpsuite  # Manual testing tool

# API token testing
jwt_tool <token>  # If using JWT
```

**Success Criteria**:
- No authentication bypass
- Rate limiting effective
- Sessions properly secured

---

### 4.2 Authorization Security

**Test Cases**:
- [ ] Permission enforcement
  - [ ] Unauthorized API access blocked
  - [ ] Privilege escalation prevented
  - [ ] Cross-user access denied
- [ ] RBAC testing
  - [ ] Admin can do everything
  - [ ] User has limited access
  - [ ] Read-only cannot modify
- [ ] VM isolation
  - [ ] VMs cannot access host
  - [ ] VMs cannot access each other (unless intended)
  - [ ] Container escape prevention

**Success Criteria**:
- All authorization checks pass
- No privilege escalation possible
- Complete VM isolation

---

### 4.3 Network Security

**Test Cases**:
- [ ] Firewall effectiveness
  - [ ] Block unwanted traffic
  - [ ] Allow authorized traffic
  - [ ] Port filtering
  - [ ] IP whitelisting/blacklisting
- [ ] TLS/SSL
  - [ ] HTTPS for API (if implemented)
  - [ ] Certificate validation
  - [ ] Strong cipher suites
- [ ] Network isolation
  - [ ] VLAN separation
  - [ ] VM network isolation
  - [ ] Management network isolation

**Success Criteria**:
- Firewall rules enforced
- TLS properly configured
- Networks properly isolated

---

### 4.4 Vulnerability Scanning

**Scans to Perform**:
- [ ] Dependency audit
  ```bash
  cargo audit
  npm audit  # For web UI
  ```
- [ ] Static analysis
  ```bash
  cargo clippy -- -D warnings
  cargo deny check
  ```
- [ ] Dynamic analysis
  ```bash
  OWASP ZAP or similar
  ```
- [ ] Penetration testing
  - [ ] External pen test
  - [ ] Internal pen test

**Success Criteria**:
- No critical vulnerabilities
- Medium/low vulns documented
- Remediation plan for findings

---

## Phase 5: Reliability Testing

### 5.1 Failover Testing

**Test Scenarios**:
- [ ] Service restart
  - [ ] Graceful restart
  - [ ] Hard restart (kill -9)
  - [ ] Verify VM survival
- [ ] Database failure
  - [ ] SQLite corruption
  - [ ] Recovery from backup
- [ ] Network failure
  - [ ] Disconnect network
  - [ ] Verify reconnection
  - [ ] Check data consistency
- [ ] Disk failure
  - [ ] Disk full scenario
  - [ ] I/O errors
  - [ ] Failover to secondary disk

**Success Criteria**:
- Services recover automatically
- No data loss
- Proper error handling

---

### 5.2 Long-Running Stability

**Test Scenarios**:
- [ ] 7-day soak test
  - [ ] System stays up
  - [ ] No memory leaks
  - [ ] No resource exhaustion
- [ ] Continuous operation
  - [ ] 24/7 VM uptime
  - [ ] Periodic tasks run correctly
  - [ ] Log rotation works

**Metrics to Monitor**:
```
Memory usage trend (should be flat)
File descriptor count (should not grow)
Database size (should grow linearly)
CPU usage (should be stable)
```

**Success Criteria**:
- System stable for 7+ days
- No memory leaks detected
- All services functional

---

## Phase 6: User Acceptance Testing

### 6.1 Real-World Scenarios

**Scenario 1: Development Environment**
```
User: Developer
Task: Create multiple VMs for testing
Steps:
1. Create 5 VMs (web, db, cache, queue, app)
2. Configure networking between VMs
3. Deploy applications
4. Monitor performance
5. Scale up/down as needed

Success: All VMs work, developer is productive
```

**Scenario 2: Production Workload**
```
User: System Administrator
Task: Migrate production app to Horcrux
Steps:
1. Create VMs matching production specs
2. Set up networking and storage
3. Configure backups
4. Migrate application
5. Monitor for 24 hours

Success: App runs with no issues, meets SLAs
```

**Scenario 3: Learning Environment**
```
User: Student/Trainer
Task: Create lab environment for training
Steps:
1. Create template VM
2. Clone VM 20 times (one per student)
3. Students access via noVNC
4. Run exercises
5. Clean up after training

Success: Students can access VMs, exercises complete
```

---

### 6.2 Usability Testing

**Tasks for Users**:
- [ ] Create a VM (without documentation)
- [ ] Connect to console
- [ ] Resize VM resources
- [ ] Create snapshot
- [ ] Restore from snapshot
- [ ] Set up networking
- [ ] Configure backup

**Metrics to Collect**:
```
Task completion rate
Time to complete tasks
Number of errors made
User satisfaction rating (1-10)
Feature requests
```

**Success Criteria**:
- > 80% task completion rate
- User satisfaction >= 7/10
- Intuitive UI/UX
- Minimal training required

---

## Automation

### Test Automation Tools

**Integration Testing**:
```bash
#!/bin/bash
# tests/integration_test.sh

# Start API server
cargo run -p horcrux-api &
API_PID=$!
sleep 5

# Run tests
cargo test --test integration_tests

# Cleanup
kill $API_PID
```

**Load Testing**:
```python
# tests/load_test.py
import asyncio
import aiohttp

async def create_vm(session, vm_id):
    async with session.post('http://localhost:8080/api/vms', json={
        'id': vm_id,
        'name': f'test-vm-{vm_id}',
        'memory': 2048,
        'cpus': 2
    }) as resp:
        return await resp.json()

async def main():
    async with aiohttp.ClientSession() as session:
        tasks = [create_vm(session, i) for i in range(50)]
        results = await asyncio.gather(*tasks)
        print(f'Created {len(results)} VMs')

asyncio.run(main())
```

**Monitoring**:
```bash
# tests/monitor.sh
#!/bin/bash

while true; do
    echo "=== $(date) ==="

    # System metrics
    vmstat 1 1
    free -h
    df -h

    # Application metrics
    curl -s http://localhost:8080/api/health | jq .

    # VM count
    virsh list --all | wc -l

    echo ""
    sleep 60
done
```

---

## Test Reports

### Daily Status Report

**Template**:
```markdown
# Test Report - YYYY-MM-DD

## Phase: [Current Phase]

### Tests Completed Today
- [ ] Test Case 1: PASS
- [ ] Test Case 2: FAIL (see issue #123)
- [ ] Test Case 3: PASS

### Issues Found
1. Issue #123: VM fails to start with >16GB RAM
   - Severity: High
   - Status: Investigating
   - Assigned: @developer

### Metrics
- Total tests run: 45
- Pass rate: 95%
- Average API response time: 87ms
- System uptime: 3 days

### Next Steps
- Continue with Network Testing
- Fix issue #123
- Begin load testing preparation
```

---

### Final Test Report

**Template**:
```markdown
# Horcrux Production Testing - Final Report

**Version**: v0.2.0
**Test Duration**: YYYY-MM-DD to YYYY-MM-DD
**Environment**: [Hardware specs]
**Team**: [Names]

## Executive Summary
[High-level overview of testing results]

## Test Coverage
- [ ] Phase 1: Basic Functionality - 100%
- [ ] Phase 2: Integration - 95%
- [ ] Phase 3: Performance - 90%
- [ ] Phase 4: Security - 100%
- [ ] Phase 5: Reliability - 85%
- [ ] Phase 6: UAT - 90%

## Issues Found
| ID | Severity | Description | Status |
|----|----------|-------------|--------|
| #123 | High | VM crash with >16GB RAM | Fixed |
| #124 | Medium | Console lag with slow network | Fixed |
| #125 | Low | UI typo in dashboard | Fixed |

## Performance Results
- VM start time: 25s (target: <30s) ✓
- API response: 65ms (target: <100ms) ✓
- Console latency: 35ms (target: <50ms) ✓
- Max concurrent VMs: 45 (target: 25+) ✓

## Recommendation
**PASS** - Ready for production deployment with minor caveats:
- Limit VMs to 16GB RAM until issue #123 is resolved
- Recommend >= 1 Gbps network for console access
- Monitor metrics closely in first week

## Sign-off
- [ ] Technical Lead
- [ ] QA Lead
- [ ] Security Lead
- [ ] Product Manager
```

---

## Success Criteria (Overall)

### Must-Have (Block Release)
- ✅ All Phase 1 tests pass
- ✅ No critical security issues
- ✅ Performance meets targets
- ✅ No data corruption/loss

### Should-Have (Release with Notes)
- ✅ All Phase 2 tests pass
- ✅ Usability score >= 7/10
- ✅ Documentation complete

### Nice-to-Have (Post-Release)
- ✅ All Phase 3-6 tests pass
- ✅ Performance exceeds targets
- ✅ Zero open bugs

---

## Timeline

**Week 1**: Phases 1-2 (Basic + Integration)
**Week 2**: Phase 3 (Performance)
**Week 3**: Phases 4-5 (Security + Reliability)
**Week 4**: Phase 6 (UAT) + Report

**Total Duration**: 4 weeks (20 business days)

---

## Resources Needed

**Personnel**:
- 2 QA Engineers (full-time)
- 1 DevOps Engineer (part-time)
- 1 Security Engineer (part-time)
- 1 Developer (on-call for fixes)

**Infrastructure**:
- 2 test servers (production-grade hardware)
- 1 staging server (for API deployment)
- Network equipment (for network testing)
- Storage (>=1 TB for VM images)

**Budget**:
- Hardware: $5,000-$10,000 (if not available)
- Cloud resources: $500/month (alternative to hardware)
- Security tools: $1,000 (OWASP ZAP Pro, etc.)

---

**Last Updated**: 2025-10-13
**Next Review**: When production hardware is available
**Status**: 📋 Draft - Awaiting hardware availability
