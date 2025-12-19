# Horcrux Gentoo Installation Guide

This directory contains Gentoo Linux configuration files optimized for running Horcrux as a virtualization host.

## Quick Installation

### 1. Copy Configuration Files

```bash
# Copy USE flags
cp package.use/horcrux /etc/portage/package.use/

# Copy keywords (for testing packages)
cp package.accept_keywords/horcrux /etc/portage/package.accept_keywords/

# Copy package set
cp sets/horcrux /etc/portage/sets/

# Review and merge make.conf settings
cat make.conf.horcrux >> /etc/portage/make.conf  # Review before running!
```

### 2. Install Required Packages

```bash
# Update package database
emerge --sync

# Install the Horcrux package set
emerge --ask --verbose @horcrux
```

### 3. Kernel Configuration

Ensure your kernel has these options enabled:

```
# Virtualization
CONFIG_VIRTUALIZATION=y
CONFIG_KVM=y
CONFIG_KVM_INTEL=y        # For Intel CPUs
CONFIG_KVM_AMD=y          # For AMD CPUs
CONFIG_VHOST_NET=y
CONFIG_VHOST_VSOCK=y

# VirtIO (guest drivers)
CONFIG_VIRTIO=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_VIRTIO_CONSOLE=y
CONFIG_VIRTIO_BALLOON=y

# IOMMU for GPU passthrough
CONFIG_IOMMU_SUPPORT=y
CONFIG_INTEL_IOMMU=y      # For Intel
CONFIG_AMD_IOMMU=y        # For AMD

# Containers
CONFIG_NAMESPACES=y
CONFIG_CGROUPS=y
CONFIG_CGROUP_DEVICE=y
CONFIG_MEMCG=y
CONFIG_CPUSETS=y
CONFIG_OVERLAY_FS=y

# Networking
CONFIG_BRIDGE=y
CONFIG_VETH=y
CONFIG_MACVLAN=y
CONFIG_IPVLAN=y
CONFIG_VXLAN=y
CONFIG_TUN=y
CONFIG_NETFILTER=y
CONFIG_NF_TABLES=y

# Storage
CONFIG_BLK_DEV_LOOP=y
CONFIG_BLK_DEV_NBD=y
CONFIG_SCSI_VIRTIO=y
CONFIG_FUSE_FS=y
```

### 4. Enable Services

```bash
# OpenRC
rc-update add libvirtd default
rc-update add docker default
rc-update add sshd default
rc-update add nftables default

# SystemD
systemctl enable libvirtd
systemctl enable docker
systemctl enable sshd
systemctl enable nftables
```

### 5. Install Horcrux

```bash
# From source
git clone https://github.com/horcrux/horcrux.git
cd horcrux
cargo build --release

# Install binaries
install -m 755 target/release/horcrux /usr/local/bin/
install -m 755 target/release/horcrux-api /usr/local/bin/

# Install configuration
mkdir -p /etc/horcrux
cp build/configs/*.toml /etc/horcrux/

# Install service
cp build/configs/horcrux.initd /etc/init.d/horcrux
chmod +x /etc/init.d/horcrux
rc-update add horcrux default
```

## Package Categories

### Minimal Installation
Essential packages only:
```bash
emerge app-emulation/qemu app-emulation/libvirt sys-fs/lvm2
```

### Standard Installation (Recommended)
Full virtualization + containers:
```bash
emerge @horcrux
```

### Full Installation
Everything including Kubernetes:
```bash
emerge @horcrux sys-cluster/kubernetes sys-cluster/kubectl sys-cluster/helm
```

## Architecture-Specific Notes

### x86_64 (AMD64)
- Enable KVM_INTEL or KVM_AMD based on your CPU
- IOMMU requires `intel_iommu=on` or `amd_iommu=on` in kernel cmdline

### ARM64 (AArch64)
- Use `ACCEPT_KEYWORDS="~arm64"` for most packages
- Some packages may need cross-compilation

### RISC-V
- Very experimental, use `ACCEPT_KEYWORDS="~riscv"`
- Many packages not yet available

## Troubleshooting

### QEMU Won't Start VMs
```bash
# Check KVM module
modprobe kvm kvm_intel  # or kvm_amd

# Check permissions
ls -la /dev/kvm
# Should be owned by kvm group

# Add user to kvm group
usermod -aG kvm username
```

### Docker Fails to Start
```bash
# Check cgroup configuration
cat /proc/cgroups

# Ensure overlay module is loaded
modprobe overlay

# Check Docker service
rc-service docker status
journalctl -u docker  # SystemD
```

### GPU Passthrough Not Working
```bash
# Check IOMMU groups
for d in /sys/kernel/iommu_groups/*/devices/*; do
    n=${d#*/iommu_groups/*}; n=${n%%/*}
    printf 'IOMMU Group %s ' "$n"
    lspci -nns "${d##*/}"
done

# Bind GPU to VFIO
echo "options vfio-pci ids=XXXX:XXXX" > /etc/modprobe.d/vfio.conf
```

## Recommended Hardware

### Minimum
- 4 CPU cores
- 8 GB RAM
- 100 GB storage

### Recommended
- 8+ CPU cores with VT-x/AMD-V
- 32+ GB RAM
- SSD/NVMe storage
- IOMMU support for GPU passthrough

### Production
- Dual CPU with 16+ cores each
- 128+ GB ECC RAM
- Enterprise NVMe or SAN storage
- Redundant networking
- IPMI/BMC for remote management
