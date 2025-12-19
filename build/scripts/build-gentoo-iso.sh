#!/bin/bash
#
# Horcrux Gentoo-based ISO Builder
# Creates a full bootable Gentoo Linux live ISO with Horcrux pre-installed
#
# This creates a complete virtualization host OS with:
#   - Full Gentoo Linux base system (stage3)
#   - Linux kernel with KVM/virtualization support
#   - QEMU, libvirt, and complete virtualization stack
#   - Container runtimes (Docker, Podman, LXC)
#   - Storage backends (ZFS, LVM, Ceph, NFS, iSCSI)
#   - Horcrux binaries, web UI, and configuration
#   - Live boot capability (runs from RAM)
#
# Usage: sudo ./build-gentoo-iso.sh [OPTIONS]
#
# Options:
#   -a, --arch ARCH       Target architecture (amd64, arm64) [default: amd64]
#   -p, --profile PROFILE Gentoo profile (openrc, systemd, hardened) [default: openrc]
#   -t, --type TYPE       Build type (minimal, standard, full) [default: standard]
#   -o, --output DIR      Output directory [default: build/iso]
#   -m, --mirror URL      Gentoo mirror URL
#   -s, --stage3 FILE     Use existing stage3 tarball (skip download)
#   -k, --kernel CONFIG   Custom kernel config file
#   -j, --jobs N          Parallel build jobs [default: nproc]
#   --skip-kernel         Skip kernel compilation (use binary kernel)
#   --keep-work           Keep work directory after build
#   -h, --help            Show this help
#
# Examples:
#   sudo ./build-gentoo-iso.sh                          # Standard amd64 build
#   sudo ./build-gentoo-iso.sh -t full                  # Full build with all features
#   sudo ./build-gentoo-iso.sh -a arm64 -t minimal      # Minimal ARM64 build
#   sudo ./build-gentoo-iso.sh -s /path/to/stage3.tar.xz  # Use existing stage3

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$BUILD_DIR")"

# Default values
ARCH="${ARCH:-amd64}"
PROFILE="${PROFILE:-openrc}"
BUILD_TYPE="${BUILD_TYPE:-standard}"
OUTPUT_DIR="${OUTPUT_DIR:-$BUILD_DIR/iso}"
MIRROR="${MIRROR:-https://distfiles.gentoo.org}"
STAGE3_FILE=""
KERNEL_CONFIG=""
JOBS="${JOBS:-$(nproc)}"
SKIP_KERNEL="${SKIP_KERNEL:-false}"
KEEP_WORK="${KEEP_WORK:-false}"

# Get version from workspace Cargo.toml
VERSION=$(grep '^version = "' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
log_step()    { echo -e "${CYAN}[STEP]${NC} $1"; }

show_help() {
    sed -n '2,35p' "$0" | sed 's/^#//'
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -a|--arch) ARCH="$2"; shift 2 ;;
        -p|--profile) PROFILE="$2"; shift 2 ;;
        -t|--type) BUILD_TYPE="$2"; shift 2 ;;
        -o|--output) OUTPUT_DIR="$2"; shift 2 ;;
        -m|--mirror) MIRROR="$2"; shift 2 ;;
        -s|--stage3) STAGE3_FILE="$2"; shift 2 ;;
        -k|--kernel) KERNEL_CONFIG="$2"; shift 2 ;;
        -j|--jobs) JOBS="$2"; shift 2 ;;
        --skip-kernel) SKIP_KERNEL="true"; shift ;;
        --keep-work) KEEP_WORK="true"; shift ;;
        -h|--help) show_help ;;
        *) log_error "Unknown option: $1"; show_help ;;
    esac
done

# Map architecture names and set targets
case "$ARCH" in
    amd64|x86_64)
        ARCH="amd64"
        GENTOO_ARCH="amd64"
        RUST_TARGET="x86_64-unknown-linux-gnu"
        KERNEL_ARCH="x86_64"
        QEMU_SYSTEM="qemu-system-x86_64"
        GRUB_TARGET="x86_64-efi"
        ;;
    arm64|aarch64)
        ARCH="arm64"
        GENTOO_ARCH="arm64"
        RUST_TARGET="aarch64-unknown-linux-gnu"
        KERNEL_ARCH="arm64"
        QEMU_SYSTEM="qemu-system-aarch64"
        GRUB_TARGET="arm64-efi"
        ;;
    *)
        log_error "Unsupported architecture: $ARCH"
        log_error "Supported: amd64, arm64"
        exit 1
        ;;
esac

# Validate build type
case "$BUILD_TYPE" in
    minimal|standard|full) ;;
    *)
        log_error "Invalid build type: $BUILD_TYPE"
        log_error "Supported: minimal, standard, full"
        exit 1
        ;;
esac

# Work directories
WORK_DIR="$BUILD_DIR/work/gentoo-$ARCH-$BUILD_TYPE"
ROOTFS_DIR="$WORK_DIR/rootfs"
ISO_DIR="$WORK_DIR/iso"
DOWNLOAD_DIR="$BUILD_DIR/work/downloads"
ISO_NAME="horcrux-${VERSION}-gentoo-${ARCH}-${BUILD_TYPE}.iso"

# ============================================================================
# HEADER
# ============================================================================

echo ""
log_info "════════════════════════════════════════════════════════════════"
log_info "  Horcrux Gentoo ISO Builder"
log_info "════════════════════════════════════════════════════════════════"
log_info "  Architecture:    $ARCH ($RUST_TARGET)"
log_info "  Profile:         $PROFILE"
log_info "  Build Type:      $BUILD_TYPE"
log_info "  Version:         $VERSION"
log_info "  Parallel Jobs:   $JOBS"
log_info "  Output:          $OUTPUT_DIR/$ISO_NAME"
log_info "════════════════════════════════════════════════════════════════"
echo ""

# ============================================================================
# PREREQUISITE CHECKS
# ============================================================================

check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root (for chroot operations)"
        log_error "Usage: sudo $0 $*"
        exit 1
    fi
}

check_requirements() {
    log_step "Checking build requirements..."

    local missing=()

    # Required tools
    local required_tools=(
        "wget:net-misc/wget"
        "tar:app-arch/tar"
        "mksquashfs:sys-fs/squashfs-tools"
        "grub-mkrescue:sys-boot/grub"
        "cpio:app-arch/cpio"
        "chroot:sys-apps/coreutils"
        "mkfs.vfat:sys-fs/dosfstools"
    )

    for tool_pkg in "${required_tools[@]}"; do
        local tool="${tool_pkg%%:*}"
        local pkg="${tool_pkg##*:}"
        if ! command -v "$tool" &>/dev/null; then
            missing+=("$tool ($pkg)")
        fi
    done

    # Check for ISO creation tool (any of these work)
    if ! command -v xorriso &>/dev/null && \
       ! command -v mkisofs &>/dev/null && \
       ! command -v genisoimage &>/dev/null; then
        missing+=("xorriso/mkisofs (dev-libs/libisoburn or app-cdr/cdrtools)")
    fi

    # Check for Rust
    if ! command -v cargo &>/dev/null; then
        missing+=("cargo (dev-lang/rust)")
    fi

    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "Missing required tools:"
        for tool in "${missing[@]}"; do
            log_error "  - $tool"
        done
        log_error ""
        log_error "Install on Gentoo with:"
        log_error "  emerge -av squashfs-tools libisoburn grub cpio dosfstools rust"
        exit 1
    fi

    # Check available disk space (need at least 20GB for full build)
    local available_gb=$(df -BG "$BUILD_DIR" | awk 'NR==2 {gsub(/G/,"",$4); print $4}')
    local required_gb=20
    if [[ "$BUILD_TYPE" == "minimal" ]]; then
        required_gb=10
    elif [[ "$BUILD_TYPE" == "full" ]]; then
        required_gb=30
    fi

    if [[ "$available_gb" -lt "$required_gb" ]]; then
        log_warn "Low disk space: ${available_gb}GB available, ${required_gb}GB recommended"
    fi

    log_success "All requirements satisfied"
}

# ============================================================================
# STAGE3 DOWNLOAD AND EXTRACTION
# ============================================================================

download_stage3() {
    log_step "Obtaining Gentoo stage3 tarball..."

    mkdir -p "$DOWNLOAD_DIR"

    if [[ -n "$STAGE3_FILE" && -f "$STAGE3_FILE" ]]; then
        log_info "Using provided stage3: $STAGE3_FILE"
        return
    fi

    # Determine stage3 variant based on profile
    local stage3_variant=""
    case "$PROFILE" in
        openrc)   stage3_variant="openrc" ;;
        systemd)  stage3_variant="systemd" ;;
        hardened) stage3_variant="hardened-openrc" ;;
        musl)     stage3_variant="musl" ;;
        *)        stage3_variant="openrc" ;;
    esac

    # Download latest stage3 listing
    local autobuilds_url="$MIRROR/releases/$GENTOO_ARCH/autobuilds"
    local latest_file="$DOWNLOAD_DIR/latest-stage3.txt"

    log_info "Fetching latest stage3 list from $autobuilds_url..."

    # Try to get the latest file
    if ! wget -q "$autobuilds_url/latest-stage3-${GENTOO_ARCH}-${stage3_variant}.txt" -O "$latest_file" 2>/dev/null; then
        # Fallback to generic latest file
        wget -q "$autobuilds_url/latest-stage3.txt" -O "$latest_file" 2>/dev/null || true
    fi

    # Parse the stage3 path
    local stage3_path=""
    if [[ -f "$latest_file" ]]; then
        stage3_path=$(grep -E "stage3-${GENTOO_ARCH}.*${stage3_variant}.*\.tar\.(xz|bz2)" "$latest_file" 2>/dev/null | head -1 | awk '{print $1}')
    fi

    # Fallback: construct a reasonable path
    if [[ -z "$stage3_path" ]]; then
        log_warn "Could not determine latest stage3, attempting direct download..."
        stage3_path="current-stage3-${GENTOO_ARCH}-${stage3_variant}/stage3-${GENTOO_ARCH}-${stage3_variant}-*.tar.xz"
    fi

    local stage3_url="$autobuilds_url/$stage3_path"
    local stage3_filename=$(basename "$stage3_path")
    STAGE3_FILE="$DOWNLOAD_DIR/$stage3_filename"

    if [[ -f "$STAGE3_FILE" ]]; then
        log_info "Stage3 already downloaded: $STAGE3_FILE"
    else
        log_info "Downloading: $stage3_url"
        wget --progress=bar:force -O "$STAGE3_FILE" "$stage3_url" || {
            log_error "Failed to download stage3"
            log_error "You can manually download from: https://www.gentoo.org/downloads/"
            log_error "Then run: $0 -s /path/to/stage3.tar.xz"
            exit 1
        }
    fi

    log_success "Stage3 ready: $STAGE3_FILE"
}

extract_stage3() {
    log_step "Extracting stage3 to $ROOTFS_DIR..."

    # Clean and create rootfs directory
    if [[ -d "$ROOTFS_DIR" ]]; then
        log_info "Cleaning existing rootfs..."
        rm -rf "$ROOTFS_DIR"
    fi
    mkdir -p "$ROOTFS_DIR"

    # Extract with proper attributes
    log_info "Extracting (this may take a few minutes)..."
    tar xpf "$STAGE3_FILE" -C "$ROOTFS_DIR" --xattrs-include='*.*' --numeric-owner

    log_success "Stage3 extracted successfully"
}

# ============================================================================
# PORTAGE CONFIGURATION
# ============================================================================

configure_portage() {
    log_step "Configuring Portage..."

    # Copy DNS resolution
    cp -L /etc/resolv.conf "$ROOTFS_DIR/etc/"

    # Create make.conf optimized for virtualization
    cat > "$ROOTFS_DIR/etc/portage/make.conf" << 'MAKECONF'
# Horcrux Virtualization Platform - Portage Configuration
# Optimized for virtualization host systems

# Compiler flags - generic for maximum compatibility
# Use baseline instruction set for each architecture
COMMON_FLAGS="-O2 -pipe __MARCH__"
CFLAGS="${COMMON_FLAGS}"
CXXFLAGS="${COMMON_FLAGS}"
FCFLAGS="${COMMON_FLAGS}"
FFLAGS="${COMMON_FLAGS}"
RUSTFLAGS="-C target-cpu=__RUSTCPU__"

# Parallel compilation
MAKEOPTS="-j__JOBS__ -l__JOBS__"
EMERGE_DEFAULT_OPTS="--jobs=__JOBS__ --load-average=__JOBS__ --with-bdeps=y"

# USE flags for virtualization host
USE="acl caps crypt dbus filecaps hardened ipv6 kerberos lz4"
USE="${USE} nls pam pcre readline sasl seccomp ssl threads unicode zstd"

# Virtualization specific
USE="${USE} io-uring qemu kvm libvirt virt-network spice usbredir virgl vnc"

# Container support
USE="${USE} btrfs device-mapper overlay"

# Storage backends
USE="${USE} fuse lvm nfs thin zfs"

# Networking
USE="${USE} curl http2 nftables netlink"

# Disable GUI for headless server
USE="${USE} -X -gtk -gtk2 -gtk3 -qt4 -qt5 -qt6 -wayland"
USE="${USE} -pulseaudio -alsa -bluetooth -cups -gnome -kde"

# Init system
USE="${USE} -systemd openrc elogind"

# Accept all licenses
ACCEPT_LICENSE="*"

# Binary packages
FEATURES="binpkg-request-signature parallel-fetch parallel-install candy"

# Localization
L10N="en"
LINGUAS="en"

# GRUB platforms
GRUB_PLATFORMS="efi-64 pc"

# QEMU targets
QEMU_SOFTMMU_TARGETS="x86_64 aarch64 arm"
QEMU_USER_TARGETS="x86_64 aarch64 arm"

# Python
PYTHON_TARGETS="python3_11 python3_12"
PYTHON_SINGLE_TARGET="python3_12"

# Video (minimal for server)
VIDEO_CARDS="dummy fbdev"
INPUT_DEVICES="libinput"
MAKECONF

    # Replace __JOBS__ placeholder
    sed -i "s/__JOBS__/$JOBS/g" "$ROOTFS_DIR/etc/portage/make.conf"

    # Set architecture-specific compiler flags for maximum compatibility
    # Use baseline instruction sets that work on the widest range of hardware
    local march_flags=""
    local rust_cpu=""
    case "$ARCH" in
        amd64)
            # x86-64 baseline - compatible with all x86_64 CPUs (SSE, SSE2)
            march_flags="-march=x86-64 -mtune=generic"
            rust_cpu="x86-64"
            ;;
        arm64)
            # ARMv8-A baseline - compatible with all 64-bit ARM
            march_flags="-march=armv8-a -mtune=generic"
            rust_cpu="generic"
            ;;
        riscv64)
            # RISC-V 64-bit GC baseline
            march_flags="-march=rv64gc -mabi=lp64d"
            rust_cpu="generic-rv64"
            ;;
        *)
            march_flags=""
            rust_cpu="generic"
            ;;
    esac
    sed -i "s/__MARCH__/$march_flags/g" "$ROOTFS_DIR/etc/portage/make.conf"
    sed -i "s/__RUSTCPU__/$rust_cpu/g" "$ROOTFS_DIR/etc/portage/make.conf"

    # Create package.use directory and files
    mkdir -p "$ROOTFS_DIR/etc/portage/package.use"
    cat > "$ROOTFS_DIR/etc/portage/package.use/horcrux" << 'PKGUSE'
# Virtualization
app-emulation/qemu spice usb usbredir virgl sdl pin-upstream-blobs
app-emulation/qemu aio bpf curl fdt fuse io-uring multipath nfs
app-emulation/qemu numa opengl oss plugins png pulseaudio
app-emulation/libvirt qemu virt-network fuse libssh lvm nfs parted pcap
app-emulation/libvirt audit firewalld libvirtd nbd numa zfs
net-misc/spice-gtk usbredir

# Container runtimes
app-containers/docker btrfs overlay
app-containers/podman btrfs fuse
app-containers/lxc seccomp

# Storage
sys-fs/zfs rootfs
sys-fs/lvm2 lvm thin
sys-block/open-iscsi -utils

# Rust
dev-lang/rust clippy rustfmt

# Database
dev-db/sqlite secure-delete

# Kernel
sys-kernel/gentoo-sources symlink
sys-kernel/linux-firmware initramfs

# Boot
sys-boot/grub mount device-mapper

# Network
net-firewall/nftables modern-kernel json python
net-firewall/iptables conntrack netlink
net-misc/openvswitch monitor

# Security
app-crypt/gnupg ssl
dev-libs/openssl -bindist
PKGUSE

    # Create package.accept_keywords
    mkdir -p "$ROOTFS_DIR/etc/portage/package.accept_keywords"
    cat > "$ROOTFS_DIR/etc/portage/package.accept_keywords/horcrux" << 'PKGKW'
# Accept testing versions for key packages
dev-lang/rust ~amd64 ~arm64
virtual/rust ~amd64 ~arm64
sys-fs/zfs ~amd64 ~arm64
sys-fs/zfs-kmod ~amd64 ~arm64
app-containers/podman ~amd64 ~arm64
app-containers/buildah ~amd64 ~arm64
net-misc/openvswitch ~amd64 ~arm64
PKGKW

    # Create package.mask if needed
    mkdir -p "$ROOTFS_DIR/etc/portage/package.mask"

    # Create repos.conf
    mkdir -p "$ROOTFS_DIR/etc/portage/repos.conf"
    cat > "$ROOTFS_DIR/etc/portage/repos.conf/gentoo.conf" << 'REPOS'
[DEFAULT]
main-repo = gentoo

[gentoo]
location = /var/db/repos/gentoo
sync-type = webrsync
sync-webrsync-verify-signature = yes
auto-sync = yes
REPOS

    log_success "Portage configured"
}

# ============================================================================
# CHROOT MANAGEMENT
# ============================================================================

CHROOT_MOUNTED=false

mount_chroot() {
    log_step "Mounting filesystems for chroot..."

    mount --types proc /proc "$ROOTFS_DIR/proc"
    mount --rbind /sys "$ROOTFS_DIR/sys"
    mount --make-rslave "$ROOTFS_DIR/sys"
    mount --rbind /dev "$ROOTFS_DIR/dev"
    mount --make-rslave "$ROOTFS_DIR/dev"
    mount --bind /run "$ROOTFS_DIR/run" 2>/dev/null || true

    # Create pts if needed
    if [[ ! -d "$ROOTFS_DIR/dev/pts" ]]; then
        mkdir -p "$ROOTFS_DIR/dev/pts"
    fi
    mount -t devpts devpts "$ROOTFS_DIR/dev/pts" 2>/dev/null || true

    # Create shm if needed
    if [[ ! -d "$ROOTFS_DIR/dev/shm" ]]; then
        mkdir -p "$ROOTFS_DIR/dev/shm"
    fi
    mount -t tmpfs shm "$ROOTFS_DIR/dev/shm" 2>/dev/null || true

    CHROOT_MOUNTED=true
    log_success "Chroot filesystems mounted"
}

umount_chroot() {
    if [[ "$CHROOT_MOUNTED" != "true" ]]; then
        return
    fi

    log_info "Unmounting chroot filesystems..."

    # Unmount in reverse order
    umount -l "$ROOTFS_DIR/dev/shm" 2>/dev/null || true
    umount -l "$ROOTFS_DIR/dev/pts" 2>/dev/null || true
    umount -l "$ROOTFS_DIR/dev" 2>/dev/null || true
    umount -l "$ROOTFS_DIR/run" 2>/dev/null || true
    umount -l "$ROOTFS_DIR/sys" 2>/dev/null || true
    umount -l "$ROOTFS_DIR/proc" 2>/dev/null || true

    CHROOT_MOUNTED=false
    log_success "Chroot filesystems unmounted"
}

run_chroot() {
    chroot "$ROOTFS_DIR" /bin/bash -c "$1"
}

# ============================================================================
# PACKAGE INSTALLATION
# ============================================================================

install_packages() {
    log_step "Installing packages (this will take a while)..."

    # Sync portage tree
    log_info "Syncing Portage tree..."
    run_chroot "emerge-webrsync" || {
        log_warn "emerge-webrsync failed, trying emerge --sync..."
        run_chroot "emerge --sync" || true
    }

    # Update portage itself
    log_info "Updating Portage..."
    run_chroot "emerge --oneshot --quiet sys-apps/portage" || true

    # Define packages based on build type
    local base_packages=(
        # System essentials
        "sys-apps/busybox"
        "sys-apps/openrc"
        "sys-apps/dbus"
        "sys-auth/elogind"
        "sys-process/cronie"
        "app-admin/sysklogd"
        "app-admin/logrotate"

        # Bootloader
        "sys-boot/grub"

        # Filesystem tools
        "sys-fs/e2fsprogs"
        "sys-fs/xfsprogs"
        "sys-fs/dosfstools"

        # Network essentials
        "net-misc/dhcpcd"
        "net-misc/openssh"
        "net-misc/curl"
        "net-misc/wget"
        "net-dns/bind-tools"

        # Basic security
        "app-admin/sudo"
        "app-crypt/gnupg"
    )

    local virt_packages=(
        # Virtualization core
        "app-emulation/qemu"
        "app-emulation/libvirt"

        # Container runtimes
        "app-containers/docker"
        "app-containers/docker-cli"
    )

    local standard_packages=(
        # Additional containers
        "app-containers/podman"
        "app-containers/lxc"
        "app-containers/cni-plugins"

        # Storage
        "sys-fs/lvm2"
        "sys-block/open-iscsi"
        "net-fs/nfs-utils"
        "net-fs/cifs-utils"

        # Networking
        "net-firewall/nftables"
        "net-firewall/iptables"
        "net-misc/bridge-utils"

        # Monitoring
        "sys-process/htop"
        "sys-apps/lm-sensors"

        # Utilities
        "app-misc/tmux"
        "app-misc/jq"
        "app-editors/vim"
        "dev-vcs/git"
    )

    local full_packages=(
        # Advanced storage
        "sys-fs/zfs"
        "sys-fs/btrfs-progs"

        # SDN
        "net-misc/openvswitch"

        # Additional security
        "net-vpn/wireguard-tools"

        # Development
        "dev-lang/rust"
    )

    # Build package list based on build type
    local packages=("${base_packages[@]}" "${virt_packages[@]}")

    if [[ "$BUILD_TYPE" == "standard" || "$BUILD_TYPE" == "full" ]]; then
        packages+=("${standard_packages[@]}")
    fi

    if [[ "$BUILD_TYPE" == "full" ]]; then
        packages+=("${full_packages[@]}")
    fi

    # Install packages
    local total=${#packages[@]}
    local current=0

    for pkg in "${packages[@]}"; do
        current=$((current + 1))
        log_info "[$current/$total] Installing $pkg..."
        run_chroot "emerge --quiet --noreplace $pkg" 2>&1 | tail -5 || {
            log_warn "Failed to install $pkg (continuing...)"
        }
    done

    log_success "Package installation complete"
}

# ============================================================================
# KERNEL BUILD
# ============================================================================

build_kernel() {
    log_step "Building Linux kernel..."

    if [[ "$SKIP_KERNEL" == "true" ]]; then
        log_info "Skipping kernel compilation - installing binary kernel instead..."

        # Accept unstable kernel if needed (binary kernel may be masked)
        run_chroot "echo 'sys-kernel/gentoo-kernel-bin ~amd64 ~arm64' >> /etc/portage/package.accept_keywords/kernel" 2>/dev/null || true
        run_chroot "echo 'virtual/dist-kernel ~amd64 ~arm64' >> /etc/portage/package.accept_keywords/kernel" 2>/dev/null || true
        run_chroot "echo 'sys-kernel/linux-firmware linux-fw-redistributable no-source-code' >> /etc/portage/package.license" 2>/dev/null || true

        # Install firmware and binary kernel
        log_info "Installing linux-firmware..."
        run_chroot "emerge --quiet sys-kernel/linux-firmware" || log_warn "linux-firmware installation failed"

        log_info "Installing binary kernel..."
        if ! run_chroot "emerge --quiet sys-kernel/gentoo-kernel-bin"; then
            log_warn "gentoo-kernel-bin failed, trying installkernel + gentoo-kernel-bin..."
            run_chroot "emerge --quiet sys-kernel/installkernel" || true
            run_chroot "emerge --quiet sys-kernel/gentoo-kernel-bin" || {
                log_error "Failed to install binary kernel"
                log_info "Attempting to install distribution kernel as fallback..."
                run_chroot "emerge --quiet sys-kernel/gentoo-kernel" || {
                    log_error "All kernel installation methods failed"
                    return 1
                }
            }
        fi

        log_info "Binary kernel installed successfully"
        return 0
    fi

    # Install kernel sources
    run_chroot "emerge --quiet sys-kernel/gentoo-sources sys-kernel/linux-firmware"

    # Link kernel source
    run_chroot "eselect kernel set 1" || true

    # Create kernel config
    if [[ -n "$KERNEL_CONFIG" && -f "$KERNEL_CONFIG" ]]; then
        log_info "Using provided kernel config: $KERNEL_CONFIG"
        cp "$KERNEL_CONFIG" "$ROOTFS_DIR/usr/src/linux/.config"
    else
        log_info "Generating kernel config for virtualization..."

        # Start with defconfig
        run_chroot "cd /usr/src/linux && make defconfig"

        # Enable virtualization features
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable VIRTUALIZATION \
            --enable KVM \
            --enable KVM_INTEL \
            --enable KVM_AMD \
            --enable VHOST_NET \
            --enable VHOST_VSOCK \
            --enable VIRTIO \
            --enable VIRTIO_PCI \
            --enable VIRTIO_PCI_LEGACY \
            --enable VIRTIO_NET \
            --enable VIRTIO_BLK \
            --enable VIRTIO_CONSOLE \
            --enable VIRTIO_BALLOON \
            --enable VIRTIO_INPUT \
            --enable VIRTIO_MMIO \
            --enable SCSI_VIRTIO \
            --enable DRM_VIRTIO_GPU \
            --enable HW_RANDOM_VIRTIO \
            --enable 9P_FS \
            --enable 9P_FS_POSIX_ACL \
            --enable NET_9P \
            --enable NET_9P_VIRTIO"

        # Enable container features
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable NAMESPACES \
            --enable UTS_NS \
            --enable IPC_NS \
            --enable USER_NS \
            --enable PID_NS \
            --enable NET_NS \
            --enable CGROUPS \
            --enable CGROUP_CPUACCT \
            --enable CGROUP_DEVICE \
            --enable CGROUP_FREEZER \
            --enable CGROUP_SCHED \
            --enable CPUSETS \
            --enable MEMCG \
            --enable CGROUP_PIDS \
            --enable CGROUP_BPF \
            --enable BLK_CGROUP"

        # Enable network features
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable BRIDGE \
            --enable BRIDGE_NETFILTER \
            --enable VETH \
            --enable TUN \
            --enable MACVLAN \
            --enable IPVLAN \
            --enable VXLAN \
            --enable GENEVE \
            --enable NETFILTER \
            --enable NF_TABLES \
            --enable NFT_NAT \
            --enable NFT_MASQ \
            --enable IP_NF_IPTABLES \
            --enable IP_NF_NAT \
            --enable IP_NF_FILTER \
            --enable IP6_NF_IPTABLES"

        # Enable storage features
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable OVERLAY_FS \
            --enable BTRFS_FS \
            --enable XFS_FS \
            --enable EXT4_FS \
            --enable FUSE_FS \
            --enable BLK_DEV_LOOP \
            --enable BLK_DEV_NBD \
            --enable BLK_DEV_DM \
            --enable DM_THIN_PROVISIONING \
            --enable DM_SNAPSHOT \
            --enable DM_MIRROR \
            --enable DM_CRYPT \
            --enable MD \
            --enable BLK_DEV_MD \
            --enable ISCSI_TCP"

        # Enable boot/ISO features
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable SQUASHFS \
            --enable SQUASHFS_XZ \
            --enable SQUASHFS_ZSTD \
            --enable ISO9660_FS \
            --enable UDF_FS \
            --enable EFI \
            --enable EFI_STUB \
            --enable EFI_PARTITION"

        # Enable IOMMU for GPU passthrough
        run_chroot "cd /usr/src/linux && scripts/config \
            --enable IOMMU_SUPPORT \
            --enable IOMMU_API \
            --enable INTEL_IOMMU \
            --enable INTEL_IOMMU_SVM \
            --enable AMD_IOMMU \
            --enable AMD_IOMMU_V2 \
            --enable VFIO \
            --enable VFIO_PCI \
            --enable VFIO_IOMMU_TYPE1"
    fi

    # Resolve any config dependencies
    run_chroot "cd /usr/src/linux && make olddefconfig"

    # Build kernel
    log_info "Compiling kernel (this takes a while)..."
    run_chroot "cd /usr/src/linux && make -j$JOBS"

    # Install modules
    log_info "Installing kernel modules..."
    run_chroot "cd /usr/src/linux && make modules_install"

    # Install kernel
    log_info "Installing kernel..."
    run_chroot "cd /usr/src/linux && make install"

    # Generate initramfs
    log_info "Generating initramfs..."
    run_chroot "emerge --quiet sys-kernel/dracut" || true

    local kernel_version=$(run_chroot "ls /lib/modules | head -1")
    run_chroot "dracut --force --kver $kernel_version" || {
        log_warn "Dracut failed, creating basic initramfs..."
        create_basic_initramfs "$kernel_version"
    }

    log_success "Kernel built successfully"
}

create_basic_initramfs() {
    local kver="$1"
    log_info "Creating basic initramfs for kernel $kver..."

    local initramfs_dir="$WORK_DIR/initramfs"
    rm -rf "$initramfs_dir"
    mkdir -p "$initramfs_dir"/{bin,sbin,etc,proc,sys,dev,newroot,lib,lib64,usr/{bin,sbin,lib,lib64}}

    # Copy busybox
    cp "$ROOTFS_DIR/bin/busybox" "$initramfs_dir/bin/"
    chmod +x "$initramfs_dir/bin/busybox"

    # Create busybox symlinks
    for cmd in sh ash mount umount switch_root cat sleep mkdir mknod ln ls cp mv rm; do
        ln -sf /bin/busybox "$initramfs_dir/bin/$cmd"
    done

    # Copy necessary kernel modules
    if [[ -d "$ROOTFS_DIR/lib/modules/$kver" ]]; then
        mkdir -p "$initramfs_dir/lib/modules/$kver"
        # Copy essential modules for live boot
        for mod in squashfs loop overlay isofs; do
            find "$ROOTFS_DIR/lib/modules/$kver" -name "${mod}*.ko*" -exec cp {} "$initramfs_dir/lib/modules/$kver/" \; 2>/dev/null || true
        done
    fi

    # Create init script
    cat > "$initramfs_dir/init" << 'INIT'
#!/bin/sh
# Horcrux Live/Installer Boot Init

# Mount essential filesystems
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev

# Create necessary device nodes
mkdir -p /dev/pts /dev/shm
mount -t devpts devpts /dev/pts
mount -t tmpfs shm /dev/shm

clear
echo ""
echo "  _    _                                "
echo " | |  | |                               "
echo " | |__| | ___  _ __ ___ _ __ _   ___  __"
echo " |  __  |/ _ \| '__/ __| '__| | | \ \/ /"
echo " | |  | | (_) | | | (__| |  | |_| |>  <"
echo " |_|  |_|\___/|_|  \___|_|   \__,_/_/\_\\"
echo ""
echo "  Gentoo Virtualization Platform"
echo "  ================================"
echo ""

# Parse kernel command line
cmdline=$(cat /proc/cmdline)
root=""
init="/sbin/init"
autoinstall=0
livemode=0

for param in $cmdline; do
    case "$param" in
        root=*) root="${param#root=}" ;;
        init=*) init="${param#init=}" ;;
        horcrux.autoinstall=*) autoinstall="${param#horcrux.autoinstall=}" ;;
        horcrux.live=*) livemode="${param#horcrux.live=}" ;;
    esac
done

# Load necessary modules
echo "Loading kernel modules..."
modprobe squashfs 2>/dev/null || true
modprobe loop 2>/dev/null || true
modprobe overlay 2>/dev/null || true
modprobe isofs 2>/dev/null || true
modprobe sr_mod 2>/dev/null || true
modprobe usb_storage 2>/dev/null || true
modprobe ahci 2>/dev/null || true
modprobe nvme 2>/dev/null || true

# Wait for devices
echo "Waiting for devices..."
sleep 3

# Find and mount the live media
echo "Searching for installation media..."
mkdir -p /cdrom /newroot

# Try CD-ROM devices first
for dev in /dev/sr0 /dev/sr1 /dev/cdrom; do
    if [ -b "$dev" ]; then
        echo "  Trying $dev..."
        if mount -t iso9660 -o ro "$dev" /cdrom 2>/dev/null; then
            if [ -f /cdrom/rootfs.squashfs ]; then
                echo "  Found installation media on $dev"
                break
            fi
            umount /cdrom
        fi
    fi
done

# Try USB devices if CD-ROM failed
if [ ! -f /cdrom/rootfs.squashfs ]; then
    for dev in /dev/sd[a-z] /dev/sd[a-z][0-9]; do
        if [ -b "$dev" ]; then
            echo "  Trying $dev..."
            if mount -o ro "$dev" /cdrom 2>/dev/null; then
                if [ -f /cdrom/rootfs.squashfs ]; then
                    echo "  Found installation media on $dev"
                    break
                fi
                umount /cdrom
            fi
        fi
    done
fi

# Mount squashfs
if [ -f /cdrom/rootfs.squashfs ]; then
    echo ""
    echo "Mounting root filesystem..."
    mkdir -p /squashfs /tmpfs

    # Mount squashfs (read-only)
    mount -t squashfs -o loop,ro /cdrom/rootfs.squashfs /squashfs

    # Create overlay for writable root
    mount -t tmpfs -o size=75% tmpfs /tmpfs
    mkdir -p /tmpfs/upper /tmpfs/work

    # Mount overlay
    mount -t overlay overlay -o lowerdir=/squashfs,upperdir=/tmpfs/upper,workdir=/tmpfs/work /newroot

    # Move mounts
    mkdir -p /newroot/cdrom /newroot/run
    mount --move /cdrom /newroot/cdrom

    # Create marker files for boot mode
    if [ "$autoinstall" = "1" ]; then
        echo "autoinstall" > /newroot/run/horcrux-boot-mode
        echo ""
        echo "*** AUTOMATIC INSTALLATION MODE ***"
        echo "The installer will start automatically after boot."
        echo ""
    elif [ "$livemode" = "1" ]; then
        echo "live" > /newroot/run/horcrux-boot-mode
        echo ""
        echo "Starting live system..."
        echo ""
    else
        echo "installer" > /newroot/run/horcrux-boot-mode
        echo ""
        echo "Starting installer..."
        echo ""
    fi

    sleep 2

    # Cleanup
    umount /proc
    umount /sys
    umount /dev/pts
    umount /dev/shm
    umount /dev

    echo "Switching to root filesystem..."
    exec switch_root /newroot "$init"
else
    echo ""
    echo "ERROR: Could not find installation media!"
    echo ""
    echo "Please ensure the installation media is properly inserted."
    echo "Tried: CD-ROM drives, USB drives"
    echo ""
    echo "Dropping to emergency shell..."
    exec /bin/sh
fi
INIT
    chmod +x "$initramfs_dir/init"

    # Create initramfs archive
    (cd "$initramfs_dir" && find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$ROOTFS_DIR/boot/initramfs-$kver.img")

    log_success "Basic initramfs created"
}

# ============================================================================
# HORCRUX INSTALLATION
# ============================================================================

install_horcrux() {
    log_step "Installing Horcrux..."

    # Build Horcrux binaries
    log_info "Building Horcrux binaries..."
    cd "$PROJECT_ROOT"

    # Build for target architecture
    if [[ "$ARCH" == "amd64" && "$(uname -m)" == "x86_64" ]]; then
        cargo build --release
    else
        cargo build --release --target "$RUST_TARGET"
    fi

    # Determine binary location
    local target_dir="$PROJECT_ROOT/target/release"
    if [[ -d "$PROJECT_ROOT/target/$RUST_TARGET/release" ]]; then
        target_dir="$PROJECT_ROOT/target/$RUST_TARGET/release"
    fi

    # Copy binaries
    log_info "Installing Horcrux binaries..."
    install -m 755 "$target_dir/horcrux" "$ROOTFS_DIR/usr/bin/"
    install -m 755 "$target_dir/horcrux-api" "$ROOTFS_DIR/usr/bin/"

    # Strip binaries
    strip "$ROOTFS_DIR/usr/bin/horcrux" 2>/dev/null || true
    strip "$ROOTFS_DIR/usr/bin/horcrux-api" 2>/dev/null || true

    # Create configuration directories
    mkdir -p "$ROOTFS_DIR/etc/horcrux"
    mkdir -p "$ROOTFS_DIR/var/lib/horcrux"/{images,backups,templates}
    mkdir -p "$ROOTFS_DIR/var/log/horcrux"
    mkdir -p "$ROOTFS_DIR/usr/share/horcrux"/{web,docs}

    # Install configuration files
    cat > "$ROOTFS_DIR/etc/horcrux/config.toml" << 'HXCONF'
# Horcrux Configuration
# Generated by ISO builder

[server]
host = "0.0.0.0"
port = 8006
workers = 4
tls_enabled = false
# tls_cert = "/etc/horcrux/ssl/server.crt"
# tls_key = "/etc/horcrux/ssl/server.key"

[database]
path = "/var/lib/horcrux/horcrux.db"

[storage]
default_pool = "local"
pools_config = "/etc/horcrux/storage.toml"
image_dir = "/var/lib/horcrux/images"

[auth]
session_timeout = 3600
max_sessions_per_user = 5
# ldap_enabled = false
# ldap_url = "ldap://localhost:389"

[logging]
level = "info"
file = "/var/log/horcrux/horcrux.log"
max_size_mb = 100
max_backups = 5

[cluster]
node_name = "horcrux-node"
enable_ha = false
# cluster_secret = ""

[monitoring]
enable = true
collection_interval = 30
retention_days = 30
HXCONF

    # Storage pools configuration
    cat > "$ROOTFS_DIR/etc/horcrux/storage.toml" << 'STCONF'
# Storage Pool Configuration

[[pools]]
name = "local"
type = "directory"
path = "/var/lib/horcrux/images"
enabled = true

# ZFS pool (uncomment and configure)
# [[pools]]
# name = "zfs-pool"
# type = "zfs"
# pool = "tank/vms"
# enabled = false

# LVM pool (uncomment and configure)
# [[pools]]
# name = "lvm-pool"
# type = "lvm"
# vg_name = "vg_vms"
# enabled = false
STCONF

    # Install OpenRC init script
    cat > "$ROOTFS_DIR/etc/init.d/horcrux" << 'INITSCRIPT'
#!/sbin/openrc-run
# Horcrux Virtualization Platform

name="Horcrux"
description="Horcrux Virtualization Platform API Server"
command="/usr/bin/horcrux-api"
command_args=""
command_background="yes"
pidfile="/run/horcrux.pid"
command_user="root"
directory="/var/lib/horcrux"
output_log="/var/log/horcrux/horcrux.log"
error_log="/var/log/horcrux/horcrux.log"

depend() {
    need net localmount
    after libvirtd docker firewall
    use dns logger
}

start_pre() {
    checkpath -d -m 0755 -o root:root /var/lib/horcrux
    checkpath -d -m 0755 -o root:root /var/log/horcrux
    checkpath -d -m 0755 -o root:root /run
}

start_post() {
    einfo "Horcrux API available at http://localhost:8006"
}
INITSCRIPT
    chmod +x "$ROOTFS_DIR/etc/init.d/horcrux"

    # Install web UI if built
    if [[ -d "$PROJECT_ROOT/horcrux-api/horcrux-ui/dist" ]]; then
        log_info "Installing web UI..."
        cp -r "$PROJECT_ROOT/horcrux-api/horcrux-ui/dist"/* "$ROOTFS_DIR/usr/share/horcrux/web/"
    fi

    # Install the installer script
    log_info "Installing system installer..."
    install -m 755 "$BUILD_DIR/installer/horcrux-installer.sh" "$ROOTFS_DIR/usr/bin/horcrux-installer"
    install -m 755 "$BUILD_DIR/installer/horcrux-boot-handler.sh" "$ROOTFS_DIR/usr/bin/horcrux-boot-handler"

    # Create installer desktop entry and menu item
    mkdir -p "$ROOTFS_DIR/usr/share/applications"
    cat > "$ROOTFS_DIR/usr/share/applications/horcrux-installer.desktop" << 'DESKTOP'
[Desktop Entry]
Name=Install Horcrux
Comment=Install Horcrux to disk
Exec=/usr/bin/horcrux-installer
Icon=system-software-install
Terminal=true
Type=Application
Categories=System;
DESKTOP

    # Create OpenRC init script for boot mode handler
    cat > "$ROOTFS_DIR/etc/init.d/horcrux-boot" << 'BOOTINIT'
#!/sbin/openrc-run
# Horcrux Boot Mode Handler

name="Horcrux Boot Handler"
description="Handle Horcrux boot mode (installer/live)"
command="/usr/bin/horcrux-boot-handler"
command_background="no"

depend() {
    need localmount
    before local
    after bootmisc
}

start() {
    ebegin "Checking Horcrux boot mode"

    # Check boot mode file
    if [ -f /run/horcrux-boot-mode ]; then
        local mode=$(cat /run/horcrux-boot-mode)
        einfo "Boot mode: $mode"

        case "$mode" in
            installer|autoinstall)
                # Don't start normal services, run installer
                einfo "Starting installer on TTY1..."
                # Run in background so init can continue
                /usr/bin/horcrux-boot-handler &
                ;;
            live)
                einfo "Live mode - normal boot"
                ;;
        esac
    fi

    eend 0
}
BOOTINIT
    chmod +x "$ROOTFS_DIR/etc/init.d/horcrux-boot"

    # Create local.d script as backup method
    mkdir -p "$ROOTFS_DIR/etc/local.d"
    cat > "$ROOTFS_DIR/etc/local.d/horcrux-installer.start" << 'LOCALD'
#!/bin/bash
# Start installer if in installer boot mode

BOOT_MODE_FILE="/run/horcrux-boot-mode"

if [[ -f "$BOOT_MODE_FILE" ]]; then
    mode=$(cat "$BOOT_MODE_FILE")

    case "$mode" in
        installer)
            # Start interactive installer on TTY1
            openvt -s -w -- /usr/bin/horcrux-installer
            ;;
        autoinstall)
            # Find first disk and run automatic install
            for disk in /dev/sda /dev/vda /dev/nvme0n1; do
                if [[ -b "$disk" ]]; then
                    openvt -s -w -- /usr/bin/horcrux-installer --disk "$disk" --auto
                    break
                fi
            done
            ;;
    esac
fi
LOCALD
    chmod +x "$ROOTFS_DIR/etc/local.d/horcrux-installer.start"

    # Enable services
    log_info "Enabling services..."
    run_chroot "rc-update add horcrux-boot boot" || true
    run_chroot "rc-update add local default" || true
    run_chroot "rc-update add horcrux default" || true
    run_chroot "rc-update add libvirtd default" || true
    run_chroot "rc-update add docker default" || true
    run_chroot "rc-update add sshd default" || true
    run_chroot "rc-update add dhcpcd default" || true
    run_chroot "rc-update add dbus default" || true
    run_chroot "rc-update add elogind boot" || true
    run_chroot "rc-update add sysklogd boot" || true
    run_chroot "rc-update add cronie default" || true

    log_success "Horcrux installed"
}

# ============================================================================
# SYSTEM CONFIGURATION
# ============================================================================

configure_system() {
    log_step "Configuring system..."

    # Set hostname
    echo "horcrux" > "$ROOTFS_DIR/etc/hostname"

    # Configure hosts
    cat > "$ROOTFS_DIR/etc/hosts" << 'HOSTS'
127.0.0.1       localhost
::1             localhost
127.0.1.1       horcrux.localdomain horcrux
HOSTS

    # Set timezone
    run_chroot "ln -sf /usr/share/zoneinfo/UTC /etc/localtime" || true

    # Configure locale
    echo 'LANG="en_US.UTF-8"' > "$ROOTFS_DIR/etc/locale.conf"
    echo 'en_US.UTF-8 UTF-8' >> "$ROOTFS_DIR/etc/locale.gen"
    run_chroot "locale-gen" || true

    # Set root password (horcrux)
    log_info "Setting root password to 'horcrux'..."
    run_chroot "echo 'root:horcrux' | chpasswd"

    # Create horcrux user with available groups
    # Build group list dynamically based on what exists
    log_info "Creating horcrux user..."
    local user_groups="wheel"
    for grp in kvm libvirt docker audio video plugdev; do
        if run_chroot "getent group $grp" &>/dev/null; then
            user_groups="$user_groups,$grp"
        fi
    done
    log_info "Adding user to groups: $user_groups"
    run_chroot "useradd -m -G $user_groups -s /bin/bash horcrux" || true
    run_chroot "echo 'horcrux:horcrux' | chpasswd" || true

    # Configure sudo
    mkdir -p "$ROOTFS_DIR/etc/sudoers.d"
    echo '%wheel ALL=(ALL) ALL' > "$ROOTFS_DIR/etc/sudoers.d/wheel"
    chmod 440 "$ROOTFS_DIR/etc/sudoers.d/wheel"

    # Configure SSH
    sed -i 's/#PermitRootLogin.*/PermitRootLogin yes/' "$ROOTFS_DIR/etc/ssh/sshd_config" || true
    sed -i 's/#PasswordAuthentication.*/PasswordAuthentication yes/' "$ROOTFS_DIR/etc/ssh/sshd_config" || true

    # OS release info
    cat > "$ROOTFS_DIR/etc/os-release" << OSREL
NAME="Horcrux"
VERSION="$VERSION"
ID=horcrux
ID_LIKE=gentoo
VERSION_ID="$VERSION"
PRETTY_NAME="Horcrux Virtualization Platform $VERSION"
HOME_URL="https://github.com/horcrux/horcrux"
BUG_REPORT_URL="https://github.com/horcrux/horcrux/issues"
BUILD_TYPE="$BUILD_TYPE"
OSREL

    # MOTD
    cat > "$ROOTFS_DIR/etc/motd" << 'MOTD'

  _    _
 | |  | |
 | |__| | ___  _ __ ___ _ __ _   ___  __
 |  __  |/ _ \| '__/ __| '__| | | \ \/ /
 | |  | | (_) | | | (__| |  | |_| |>  <
 |_|  |_|\___/|_|  \___|_|   \__,_/_/\_\

  Gentoo-based Virtualization Platform - Live System

  =====================================================
  INSTALL TO DISK:  Run 'horcrux-installer' as root
  =====================================================

  Web UI:    http://localhost:8006
  CLI:       horcrux --help
  Docs:      /usr/share/horcrux/docs

  Default credentials: root / horcrux

  This is a live system running from RAM. Changes will
  not persist across reboots. To install permanently,
  run the installer.

MOTD

    log_success "System configured"
}

# ============================================================================
# ISO CREATION
# ============================================================================

create_iso() {
    log_step "Creating bootable ISO..."

    # Create ISO directory structure
    rm -rf "$ISO_DIR"
    mkdir -p "$ISO_DIR"/{boot/grub,EFI/BOOT,isolinux}
    mkdir -p "$OUTPUT_DIR"

    # Find kernel and initramfs
    local kernel_file=$(ls "$ROOTFS_DIR/boot/vmlinuz"* 2>/dev/null | head -1)
    local initrd_file=$(ls "$ROOTFS_DIR/boot/initramfs"* 2>/dev/null | head -1)

    if [[ ! -f "$kernel_file" ]]; then
        kernel_file=$(ls "$ROOTFS_DIR/boot/kernel"* 2>/dev/null | head -1)
    fi

    if [[ -z "$kernel_file" || ! -f "$kernel_file" ]]; then
        log_error "No kernel found in $ROOTFS_DIR/boot/"
        ls -la "$ROOTFS_DIR/boot/" || true
        exit 1
    fi

    log_info "Using kernel: $kernel_file"
    cp "$kernel_file" "$ISO_DIR/boot/vmlinuz"

    if [[ -f "$initrd_file" ]]; then
        log_info "Using initramfs: $initrd_file"
        cp "$initrd_file" "$ISO_DIR/boot/initrd.img"
    else
        log_warn "No initramfs found, creating minimal one..."
        local kver=$(basename "$kernel_file" | sed 's/vmlinuz-//')
        create_basic_initramfs "$kver"
        cp "$ROOTFS_DIR/boot/initramfs-$kver.img" "$ISO_DIR/boot/initrd.img"
    fi

    # Create squashfs from rootfs
    log_info "Creating squashfs (this may take a while)..."
    mksquashfs "$ROOTFS_DIR" "$ISO_DIR/rootfs.squashfs" \
        -comp xz -b 1M -Xdict-size 100% \
        -e "$ROOTFS_DIR/usr/src" \
        -e "$ROOTFS_DIR/var/cache" \
        -e "$ROOTFS_DIR/var/tmp" \
        -e "$ROOTFS_DIR/tmp"

    # Create GRUB configuration
    cat > "$ISO_DIR/boot/grub/grub.cfg" << GRUBCFG
set timeout=30
set default=0

insmod all_video
insmod gfxterm
insmod png

if loadfont /boot/grub/fonts/unicode.pf2 ; then
    set gfxmode=auto
    terminal_output gfxterm
fi

set menu_color_normal=white/black
set menu_color_highlight=black/light-gray

# Header
echo ""
echo "  Horcrux Virtualization Platform $VERSION"
echo "  ========================================"
echo ""

menuentry "Install Horcrux to Disk" {
    linux /boot/vmlinuz quiet splash horcrux.autoinstall=0
    initrd /boot/initrd.img
}

menuentry "Install Horcrux (Automatic - ERASES FIRST DISK)" {
    linux /boot/vmlinuz quiet horcrux.autoinstall=1
    initrd /boot/initrd.img
}

menuentry "--" {
    true
}

menuentry "Live System (Try without installing)" {
    linux /boot/vmlinuz quiet splash horcrux.live=1
    initrd /boot/initrd.img
}

menuentry "Live System (Debug Mode)" {
    linux /boot/vmlinuz debug earlyprintk=serial,ttyS0,115200 horcrux.live=1
    initrd /boot/initrd.img
}

menuentry "--" {
    true
}

menuentry "Advanced Options >" {
    configfile /boot/grub/advanced.cfg
}

menuentry "Reboot" {
    reboot
}

menuentry "Power Off" {
    halt
}
GRUBCFG

    # Create advanced options menu
    cat > "$ISO_DIR/boot/grub/advanced.cfg" << ADVGRUB
set timeout=-1

menuentry "< Back to Main Menu" {
    configfile /boot/grub/grub.cfg
}

menuentry "--" {
    true
}

menuentry "Install - Custom Disk Selection" {
    linux /boot/vmlinuz quiet horcrux.autoinstall=0
    initrd /boot/initrd.img
}

menuentry "Install - Safe Graphics Mode" {
    linux /boot/vmlinuz nomodeset horcrux.autoinstall=0
    initrd /boot/initrd.img
}

menuentry "Install - Serial Console (ttyS0)" {
    linux /boot/vmlinuz console=ttyS0,115200n8 horcrux.autoinstall=0
    initrd /boot/initrd.img
}

menuentry "--" {
    true
}

menuentry "Live - Safe Graphics Mode" {
    linux /boot/vmlinuz nomodeset acpi=off horcrux.live=1
    initrd /boot/initrd.img
}

menuentry "Live - Serial Console (ttyS0)" {
    linux /boot/vmlinuz console=ttyS0,115200n8 horcrux.live=1
    initrd /boot/initrd.img
}

menuentry "Emergency Shell" {
    linux /boot/vmlinuz init=/bin/sh
    initrd /boot/initrd.img
}

menuentry "--" {
    true
}

menuentry "Memory Test (if available)" {
    linux16 /boot/memtest86+.bin
}
ADVGRUB

    # Create EFI boot image
    log_info "Creating EFI boot image..."
    mkdir -p "$ISO_DIR/EFI/BOOT"

    # Copy GRUB EFI binary
    if [[ -f "/usr/lib/grub/x86_64-efi/grub.efi" ]]; then
        cp /usr/lib/grub/x86_64-efi/grub.efi "$ISO_DIR/EFI/BOOT/BOOTX64.EFI"
    else
        # Generate GRUB EFI
        grub-mkstandalone \
            --format=x86_64-efi \
            --output="$ISO_DIR/EFI/BOOT/BOOTX64.EFI" \
            --locales="" \
            --fonts="" \
            "boot/grub/grub.cfg=$ISO_DIR/boot/grub/grub.cfg" 2>/dev/null || true
    fi

    # Create EFI system partition image
    local efi_size=4  # MB
    dd if=/dev/zero of="$ISO_DIR/efi.img" bs=1M count=$efi_size 2>/dev/null
    mkfs.vfat "$ISO_DIR/efi.img" >/dev/null
    mmd -i "$ISO_DIR/efi.img" ::EFI
    mmd -i "$ISO_DIR/efi.img" ::EFI/BOOT
    mcopy -i "$ISO_DIR/efi.img" "$ISO_DIR/EFI/BOOT/BOOTX64.EFI" ::EFI/BOOT/ 2>/dev/null || true

    # Create the ISO
    log_info "Creating ISO image..."

    xorriso -as mkisofs \
        -iso-level 3 \
        -full-iso9660-filenames \
        -volid "HORCRUX_$VERSION" \
        -appid "Horcrux Virtualization Platform" \
        -publisher "Horcrux Project" \
        -eltorito-boot boot/grub/i386-pc/eltorito.img \
        -no-emul-boot \
        -boot-load-size 4 \
        -boot-info-table \
        --eltorito-catalog boot/grub/boot.cat \
        --grub2-boot-info \
        --grub2-mbr /usr/lib/grub/i386-pc/boot_hybrid.img \
        -eltorito-alt-boot \
        -e efi.img \
        -no-emul-boot \
        -isohybrid-gpt-basdat \
        -output "$OUTPUT_DIR/$ISO_NAME" \
        "$ISO_DIR" 2>/dev/null || {

        # Fallback to simpler ISO creation
        log_warn "Advanced ISO creation failed, using grub-mkrescue..."
        grub-mkrescue -o "$OUTPUT_DIR/$ISO_NAME" "$ISO_DIR" -- \
            -volid "HORCRUX_$VERSION" \
            -appid "Horcrux Virtualization Platform"
    }

    # Generate checksums
    cd "$OUTPUT_DIR"
    sha256sum "$ISO_NAME" > "${ISO_NAME}.sha256"
    sha512sum "$ISO_NAME" > "${ISO_NAME}.sha512"

    log_success "ISO created: $OUTPUT_DIR/$ISO_NAME"
}

# ============================================================================
# CLEANUP
# ============================================================================

cleanup() {
    log_info "Cleaning up..."
    umount_chroot

    if [[ "$KEEP_WORK" != "true" ]]; then
        log_info "Removing work directory..."
        rm -rf "$WORK_DIR"
    fi
}

trap cleanup EXIT

# ============================================================================
# MAIN
# ============================================================================

main() {
    local start_time=$(date +%s)

    check_root
    check_requirements
    download_stage3
    extract_stage3
    configure_portage
    mount_chroot
    install_packages
    build_kernel
    install_horcrux
    configure_system
    umount_chroot
    create_iso

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    local hours=$((duration / 3600))
    local minutes=$(((duration % 3600) / 60))
    local seconds=$((duration % 60))

    local iso_size=$(du -h "$OUTPUT_DIR/$ISO_NAME" | cut -f1)

    echo ""
    log_info "════════════════════════════════════════════════════════════════"
    log_success "Horcrux Gentoo ISO Build Complete!"
    log_info "════════════════════════════════════════════════════════════════"
    log_info "  ISO:        $OUTPUT_DIR/$ISO_NAME"
    log_info "  Size:       $iso_size"
    log_info "  Build Time: ${hours}h ${minutes}m ${seconds}s"
    log_info ""
    log_info "  SHA256:     $(cat "$OUTPUT_DIR/${ISO_NAME}.sha256" | cut -d' ' -f1)"
    log_info ""
    log_info "  Test with QEMU:"
    log_info "    $QEMU_SYSTEM -cdrom $OUTPUT_DIR/$ISO_NAME -m 4G -enable-kvm"
    log_info ""
    log_info "  Default Credentials:"
    log_info "    Username: root / horcrux"
    log_info "    Password: horcrux"
    log_info ""
    log_info "  After boot, access Horcrux at: http://<ip>:8006"
    log_info "════════════════════════════════════════════════════════════════"
}

main "$@"
