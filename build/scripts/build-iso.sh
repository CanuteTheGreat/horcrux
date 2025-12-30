#!/bin/bash
#
# Horcrux ISO Builder
# Builds bootable ISO images for Horcrux Virtualization Platform
#
# Supported architectures:
#   - x86_64 (amd64)
#   - aarch64 (arm64)
#   - riscv64
#
# Usage: ./build-iso.sh [OPTIONS]
#
# Options:
#   -a, --arch ARCH       Target architecture (x86_64, aarch64, riscv64)
#   -o, --output DIR      Output directory for ISO
#   -v, --version VER     Version string (default: from Cargo.toml)
#   -t, --type TYPE       Build type: minimal, standard, full (default: standard)
#   -c, --clean           Clean build artifacts before building
#   -j, --jobs N          Number of parallel jobs (default: nproc)
#   -h, --help            Show this help message
#
# Examples:
#   ./build-iso.sh -a x86_64 -t standard
#   ./build-iso.sh -a aarch64 -o /tmp/iso
#   ./build-iso.sh --arch riscv64 --type minimal

set -euo pipefail

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$BUILD_DIR")"

# Default values
ARCH="${ARCH:-x86_64}"
OUTPUT_DIR="${OUTPUT_DIR:-$BUILD_DIR/iso}"
BUILD_TYPE="${BUILD_TYPE:-standard}"
CLEAN_BUILD="${CLEAN_BUILD:-false}"
JOBS="${JOBS:-$(nproc)}"
VERSION=""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

show_help() {
    head -30 "$0" | tail -25 | sed 's/^#//'
    exit 0
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -a|--arch)
            ARCH="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -v|--version)
            VERSION="$2"
            shift 2
            ;;
        -t|--type)
            BUILD_TYPE="$2"
            shift 2
            ;;
        -c|--clean)
            CLEAN_BUILD="true"
            shift
            ;;
        -j|--jobs)
            JOBS="$2"
            shift 2
            ;;
        -h|--help)
            show_help
            ;;
        *)
            log_error "Unknown option: $1"
            show_help
            ;;
    esac
done

# Validate architecture
case "$ARCH" in
    x86_64|amd64)
        ARCH="x86_64"
        RUST_TARGET="x86_64-unknown-linux-gnu"
        KERNEL_ARCH="x86_64"
        GRUB_TARGET="x86_64-efi"
        QEMU_SYSTEM="qemu-system-x86_64"
        ;;
    aarch64|arm64)
        ARCH="aarch64"
        RUST_TARGET="aarch64-unknown-linux-gnu"
        KERNEL_ARCH="arm64"
        GRUB_TARGET="arm64-efi"
        QEMU_SYSTEM="qemu-system-aarch64"
        ;;
    riscv64|riscv)
        ARCH="riscv64"
        RUST_TARGET="riscv64gc-unknown-linux-gnu"
        KERNEL_ARCH="riscv"
        GRUB_TARGET="riscv64-efi"
        QEMU_SYSTEM="qemu-system-riscv64"
        ;;
    *)
        log_error "Unsupported architecture: $ARCH"
        log_error "Supported: x86_64, aarch64, riscv64"
        exit 1
        ;;
esac

# Validate build type
case "$BUILD_TYPE" in
    minimal|standard|full)
        ;;
    *)
        log_error "Invalid build type: $BUILD_TYPE"
        log_error "Supported: minimal, standard, full"
        exit 1
        ;;
esac

# Get version from Cargo.toml if not specified
if [[ -z "$VERSION" ]]; then
    VERSION=$(grep '^version = "' "$PROJECT_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

# Build directories
WORK_DIR="$BUILD_DIR/work/$ARCH"
ROOTFS_DIR="$WORK_DIR/rootfs"
ISO_DIR="$WORK_DIR/iso"
ISO_NAME="horcrux-${VERSION}-${ARCH}-${BUILD_TYPE}.iso"

log_info "=============================================="
log_info "Horcrux ISO Builder"
log_info "=============================================="
log_info "Architecture:  $ARCH"
log_info "Rust target:   $RUST_TARGET"
log_info "Build type:    $BUILD_TYPE"
log_info "Version:       $VERSION"
log_info "Output:        $OUTPUT_DIR/$ISO_NAME"
log_info "Jobs:          $JOBS"
log_info "=============================================="

# Check for required tools
check_requirements() {
    log_info "Checking build requirements..."

    local missing=()

    # Required tools
    local tools=(
        "cargo"
        "rustc"
        "mksquashfs"
        "grub-mkrescue"
        "grub-mkimage"
        "cpio"
    )

    # Check for mkfs.vfat or mkfs.fat for EFI
    if ! command -v mkfs.vfat &> /dev/null && ! command -v mkfs.fat &> /dev/null; then
        log_warn "mkfs.vfat/mkfs.fat not found - EFI boot may not work"
        log_warn "Install with: emerge -av sys-fs/dosfstools"
    fi

    for tool in "${tools[@]}"; do
        if ! command -v "$tool" &> /dev/null; then
            missing+=("$tool")
        fi
    done

    # Check for ISO creation tool (xorriso or mkisofs)
    if ! command -v xorriso &> /dev/null && ! command -v mkisofs &> /dev/null; then
        missing+=("xorriso or mkisofs")
    fi

    # Check for cross-compilation tools if needed
    if [[ "$ARCH" != "$(uname -m)" ]]; then
        case "$ARCH" in
            aarch64)
                if ! command -v aarch64-unknown-linux-gnu-gcc &> /dev/null && \
                   ! command -v aarch64-linux-gnu-gcc &> /dev/null; then
                    missing+=("aarch64 cross-compiler")
                fi
                ;;
            riscv64)
                if ! command -v riscv64-unknown-linux-gnu-gcc &> /dev/null && \
                   ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
                    missing+=("riscv64 cross-compiler")
                fi
                ;;
        esac
    fi

    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "Missing required tools:"
        for tool in "${missing[@]}"; do
            log_error "  - $tool"
        done
        log_error ""
        log_error "On Gentoo, install with:"
        log_error "  emerge -av squashfs-tools libisoburn grub sys-boot/grub cpio"
        log_error ""
        log_error "For cross-compilation:"
        log_error "  crossdev -t aarch64-unknown-linux-gnu"
        log_error "  crossdev -t riscv64-unknown-linux-gnu"
        exit 1
    fi

    log_success "All requirements satisfied"
}

# Clean build artifacts
clean_build() {
    if [[ "$CLEAN_BUILD" == "true" ]]; then
        log_info "Cleaning build artifacts..."
        rm -rf "$WORK_DIR"
        log_success "Cleaned $WORK_DIR"
    fi
}

# Install Rust target
setup_rust_target() {
    log_info "Setting up Rust target: $RUST_TARGET"

    # Check if rustup is available
    if command -v rustup &> /dev/null; then
        if ! rustup target list --installed | grep -q "$RUST_TARGET"; then
            rustup target add "$RUST_TARGET"
        fi
    else
        # System Rust - check if target is supported
        if [[ "$ARCH" == "$(uname -m)" ]] || [[ "$ARCH" == "x86_64" && "$(uname -m)" == "x86_64" ]]; then
            log_info "Using system Rust for native target"
        else
            log_warn "Cross-compilation may require additional setup without rustup"
        fi
    fi

    log_success "Rust target ready"
}

# Build Horcrux binaries
build_binaries() {
    log_info "Building Horcrux binaries for $ARCH..."

    cd "$PROJECT_ROOT"

    # Set cross-compilation environment if needed
    if [[ "$ARCH" != "$(uname -m)" ]]; then
        case "$ARCH" in
            aarch64)
                export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="aarch64-unknown-linux-gnu-gcc"
                export CC_aarch64_unknown_linux_gnu="aarch64-unknown-linux-gnu-gcc"
                export CXX_aarch64_unknown_linux_gnu="aarch64-unknown-linux-gnu-g++"
                ;;
            riscv64)
                export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER="riscv64-unknown-linux-gnu-gcc"
                export CC_riscv64gc_unknown_linux_gnu="riscv64-unknown-linux-gnu-gcc"
                export CXX_riscv64gc_unknown_linux_gnu="riscv64-unknown-linux-gnu-g++"
                ;;
        esac
    fi

    # Build with appropriate features based on build type
    local features=""
    case "$BUILD_TYPE" in
        minimal)
            features=""
            ;;
        standard)
            features=""
            ;;
        full)
            features="--features kubernetes"
            ;;
    esac

    cargo build --release --target "$RUST_TARGET" -j "$JOBS" $features

    log_success "Binaries built successfully"
}

# Create root filesystem
create_rootfs() {
    log_info "Creating root filesystem..."

    mkdir -p "$ROOTFS_DIR"/{bin,sbin,lib,lib64,usr/{bin,sbin,lib,lib64,share},etc,var,tmp,run,proc,sys,dev}
    mkdir -p "$ROOTFS_DIR"/etc/{horcrux,ssl/certs}
    mkdir -p "$ROOTFS_DIR"/var/{lib/horcrux,log,run}
    mkdir -p "$ROOTFS_DIR"/usr/share/horcrux/{web,docs}

    # Copy Horcrux binaries
    local target_dir="$PROJECT_ROOT/target/$RUST_TARGET/release"

    cp "$target_dir/horcrux" "$ROOTFS_DIR/usr/bin/"
    cp "$target_dir/horcrux-api" "$ROOTFS_DIR/usr/bin/"

    # Copy web UI if exists
    if [[ -d "$PROJECT_ROOT/horcrux-api/horcrux-ui/dist" ]]; then
        cp -r "$PROJECT_ROOT/horcrux-api/horcrux-ui/dist"/* "$ROOTFS_DIR/usr/share/horcrux/web/"
    fi

    # Strip binaries
    if [[ "$ARCH" == "$(uname -m)" ]]; then
        strip "$ROOTFS_DIR/usr/bin/horcrux" || true
        strip "$ROOTFS_DIR/usr/bin/horcrux-api" || true
    else
        case "$ARCH" in
            aarch64)
                aarch64-unknown-linux-gnu-strip "$ROOTFS_DIR/usr/bin/horcrux" 2>/dev/null || true
                aarch64-unknown-linux-gnu-strip "$ROOTFS_DIR/usr/bin/horcrux-api" 2>/dev/null || true
                ;;
            riscv64)
                riscv64-unknown-linux-gnu-strip "$ROOTFS_DIR/usr/bin/horcrux" 2>/dev/null || true
                riscv64-unknown-linux-gnu-strip "$ROOTFS_DIR/usr/bin/horcrux-api" 2>/dev/null || true
                ;;
        esac
    fi

    log_success "Root filesystem created"
}

# Create configuration files
create_configs() {
    log_info "Creating configuration files..."

    # Main Horcrux configuration
    cat > "$ROOTFS_DIR/etc/horcrux/config.toml" << 'EOF'
# Horcrux Configuration
# Generated by ISO builder

[server]
host = "0.0.0.0"
port = 8006
workers = 4

[database]
path = "/var/lib/horcrux/horcrux.db"

[storage]
default_pool = "local"
pools_config = "/etc/horcrux/storage.toml"

[auth]
session_timeout = 3600
max_sessions_per_user = 5

[logging]
level = "info"
file = "/var/log/horcrux/horcrux.log"

[cluster]
node_name = "horcrux-node"
enable_ha = false

[monitoring]
enable = true
collection_interval = 30
retention_days = 30
EOF

    # Storage configuration
    cat > "$ROOTFS_DIR/etc/horcrux/storage.toml" << 'EOF'
# Storage Pool Configuration

[[pools]]
name = "local"
type = "directory"
path = "/var/lib/horcrux/images"
enabled = true

# Example ZFS pool (uncomment to enable)
# [[pools]]
# name = "zfs-pool"
# type = "zfs"
# pool = "tank/vms"
# enabled = false
EOF

    # Systemd service file
    mkdir -p "$ROOTFS_DIR/etc/systemd/system"
    cat > "$ROOTFS_DIR/etc/systemd/system/horcrux.service" << 'EOF'
[Unit]
Description=Horcrux Virtualization Platform
Documentation=https://github.com/horcrux/horcrux
After=network-online.target libvirtd.service
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/bin/horcrux-api
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
LimitNOFILE=65536
LimitNPROC=32768
Environment=RUST_LOG=info
WorkingDirectory=/var/lib/horcrux

[Install]
WantedBy=multi-user.target
EOF

    # OpenRC init script for Gentoo
    mkdir -p "$ROOTFS_DIR/etc/init.d"
    cat > "$ROOTFS_DIR/etc/init.d/horcrux" << 'EOF'
#!/sbin/openrc-run
# Horcrux OpenRC init script

name="Horcrux"
description="Horcrux Virtualization Platform"
command="/usr/bin/horcrux-api"
command_background="yes"
pidfile="/run/horcrux.pid"
command_user="root"
directory="/var/lib/horcrux"
output_log="/var/log/horcrux/horcrux.log"
error_log="/var/log/horcrux/horcrux.log"

depend() {
    need net
    after libvirtd
}

start_pre() {
    checkpath -d -m 0755 -o root:root /var/lib/horcrux
    checkpath -d -m 0755 -o root:root /var/log/horcrux
}
EOF
    chmod +x "$ROOTFS_DIR/etc/init.d/horcrux"

    # OS release info
    cat > "$ROOTFS_DIR/etc/os-release" << EOF
NAME="Horcrux"
VERSION="$VERSION"
ID=horcrux
ID_LIKE=gentoo
VERSION_ID="$VERSION"
PRETTY_NAME="Horcrux Virtualization Platform $VERSION"
HOME_URL="https://github.com/horcrux/horcrux"
BUG_REPORT_URL="https://github.com/horcrux/horcrux/issues"
EOF

    log_success "Configuration files created"
}

# Create initramfs
create_initramfs() {
    log_info "Creating initramfs..."

    local initramfs_dir="$WORK_DIR/initramfs"
    mkdir -p "$initramfs_dir"/{bin,sbin,etc,proc,sys,dev,newroot,lib,lib64}

    # Create init script
    cat > "$initramfs_dir/init" << 'EOF'
#!/bin/sh
# Horcrux initramfs init

echo "Horcrux Initramfs Starting..."

# Mount essential filesystems
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev 2>/dev/null || {
    # Fallback: create basic device nodes
    mknod /dev/console c 5 1 2>/dev/null
    mknod /dev/null c 1 3 2>/dev/null
    mknod /dev/sr0 b 11 0 2>/dev/null
}

# Parse kernel command line
cmdline=$(cat /proc/cmdline)
root=""
init="/sbin/init"

echo "Kernel cmdline: $cmdline"

for param in $cmdline; do
    case "$param" in
        root=*)
            root="${param#root=}"
            ;;
        init=*)
            init="${param#init=}"
            ;;
    esac
done

echo "Root device: $root"
echo "Init: $init"

# Wait for root device
echo "Waiting for devices..."
sleep 3

# Create mount points
mkdir -p /mnt /newroot

# Mount root filesystem
if [ -n "$root" ] && [ "$root" != "/dev/sr0" ]; then
    echo "Mounting root: $root"
    mount -o ro "$root" /newroot
else
    # Try to find squashfs on CD-ROM
    echo "Attempting to mount CD-ROM..."
    if [ -e /dev/sr0 ]; then
        mount -t iso9660 -o ro /dev/sr0 /mnt
        if [ -f /mnt/horcrux.squashfs ]; then
            echo "Found squashfs, mounting..."
            mount -t squashfs -o loop /mnt/horcrux.squashfs /newroot
        else
            echo "ERROR: horcrux.squashfs not found on CD-ROM"
            echo "Contents of /mnt:"
            ls -la /mnt 2>/dev/null || echo "(cannot list)"
            echo "Dropping to shell..."
            exec /bin/sh
        fi
    else
        echo "ERROR: /dev/sr0 not found"
        echo "Available block devices:"
        ls -la /dev/sd* /dev/sr* /dev/vd* 2>/dev/null || echo "(none found)"
        echo "Dropping to shell..."
        exec /bin/sh
    fi
fi

# Verify newroot is mounted
if [ ! -d /newroot/sbin ]; then
    echo "ERROR: Root filesystem not properly mounted"
    echo "Contents of /newroot:"
    ls -la /newroot 2>/dev/null || echo "(cannot list)"
    echo "Dropping to shell..."
    exec /bin/sh
fi

echo "Switching to root filesystem..."

# Switch to real root
umount /proc 2>/dev/null
umount /sys 2>/dev/null
umount /dev 2>/dev/null

exec switch_root /newroot "$init"
EOF
    chmod +x "$initramfs_dir/init"

    # Copy busybox (static) or individual tools with libraries
    if command -v busybox &> /dev/null && file "$(which busybox)" | grep -q "statically linked"; then
        log_info "Using statically linked busybox"
        cp "$(which busybox)" "$initramfs_dir/bin/"

        # Create symlinks for essential commands
        for cmd in sh mount umount switch_root cat sleep mknod mkdir; do
            ln -sf /bin/busybox "$initramfs_dir/bin/$cmd"
        done
        ln -sf /bin/busybox "$initramfs_dir/sbin/switch_root"
    else
        log_warn "busybox not found or not static, copying tools with libraries..."

        # Create lib directories
        mkdir -p "$initramfs_dir/lib64" "$initramfs_dir/lib"

        # Copy essential binaries from system
        local binaries=()
        for cmd in sh bash mount umount cat sleep; do
            if command -v "$cmd" &> /dev/null; then
                local bin_path="$(which $cmd)"
                cp "$bin_path" "$initramfs_dir/bin/" 2>/dev/null || true
                binaries+=("$bin_path")
            fi
        done

        # Try to get switch_root from util-linux
        if [[ -f /sbin/switch_root ]]; then
            cp /sbin/switch_root "$initramfs_dir/sbin/"
            binaries+=("/sbin/switch_root")
        elif [[ -f /usr/sbin/switch_root ]]; then
            cp /usr/sbin/switch_root "$initramfs_dir/sbin/"
            binaries+=("/usr/sbin/switch_root")
        fi

        # Copy dynamic linker
        if [[ -f /lib64/ld-linux-x86-64.so.2 ]]; then
            cp /lib64/ld-linux-x86-64.so.2 "$initramfs_dir/lib64/"
        fi

        # Copy required libraries for all binaries
        for bin in "${binaries[@]}"; do
            if [[ -f "$bin" ]]; then
                local libs
                libs=$(ldd "$bin" 2>/dev/null | grep -o '/[^ ]*' || true)
                for lib in $libs; do
                    if [[ -f "$lib" && ! -f "$initramfs_dir$lib" ]]; then
                        local lib_dir="$initramfs_dir$(dirname "$lib")"
                        mkdir -p "$lib_dir"
                        cp "$lib" "$lib_dir/" 2>/dev/null || true
                    fi
                done
            fi
        done

        # Create sh symlink to bash if needed
        if [[ -f "$initramfs_dir/bin/bash" && ! -f "$initramfs_dir/bin/sh" ]]; then
            ln -sf bash "$initramfs_dir/bin/sh"
        fi
    fi

    # Create /mnt for CD-ROM mount
    mkdir -p "$initramfs_dir/mnt"

    # Create initramfs cpio archive
    (cd "$initramfs_dir" && find . | cpio -o -H newc | gzip > "$WORK_DIR/initramfs.cpio.gz")

    log_success "Initramfs created"
}

# Find or download kernel
setup_kernel() {
    log_info "Setting up kernel..."

    # Ensure work directory exists
    mkdir -p "$WORK_DIR"

    local kernel_found=false
    local kernel_src=""

    # Try to find existing kernel on the system
    local kernel_paths=(
        "/boot/vmlinuz-$(uname -r)"
        "/boot/vmlinuz"
        "/boot/kernel-$(uname -r)"
        "/usr/src/linux/arch/${KERNEL_ARCH}/boot/bzImage"
        "/usr/src/linux/arch/${KERNEL_ARCH}/boot/Image"
    )

    for kpath in "${kernel_paths[@]}"; do
        if [[ -f "$kpath" ]]; then
            kernel_src="$kpath"
            kernel_found=true
            log_info "Found kernel at: $kpath"
            break
        fi
    done

    # Search for any vmlinuz in /boot
    if [[ "$kernel_found" != "true" ]]; then
        kernel_src=$(ls -1t /boot/vmlinuz-* 2>/dev/null | head -1)
        if [[ -n "$kernel_src" && -f "$kernel_src" ]]; then
            kernel_found=true
            log_info "Found kernel at: $kernel_src"
        fi
    fi

    # Search for kernel in /boot with different naming
    if [[ "$kernel_found" != "true" ]]; then
        kernel_src=$(ls -1t /boot/kernel-* 2>/dev/null | head -1)
        if [[ -n "$kernel_src" && -f "$kernel_src" ]]; then
            kernel_found=true
            log_info "Found kernel at: $kernel_src"
        fi
    fi

    if [[ "$kernel_found" != "true" ]]; then
        log_error "No kernel found!"
        log_error "Please ensure a kernel is installed in /boot or /usr/src/linux"
        log_error "On Gentoo: emerge -av sys-kernel/gentoo-kernel-bin"
        log_error "Or compile your own: cd /usr/src/linux && make && make install"
        exit 1
    fi

    # Copy kernel to work directory
    cp "$kernel_src" "$WORK_DIR/vmlinuz"
    log_success "Kernel ready: $kernel_src"
}

# Setup GRUB bootloader files
setup_bootloader() {
    log_info "Setting up bootloader..."

    mkdir -p "$ISO_DIR/boot/grub/i386-pc"
    mkdir -p "$ISO_DIR/boot/grub/${GRUB_TARGET}"
    mkdir -p "$ISO_DIR/EFI/BOOT"

    # Find GRUB modules directory
    local grub_lib=""
    local grub_dirs=(
        "/usr/lib/grub"
        "/usr/share/grub"
        "/usr/lib64/grub"
    )

    for gdir in "${grub_dirs[@]}"; do
        if [[ -d "$gdir/i386-pc" ]]; then
            grub_lib="$gdir"
            break
        fi
    done

    if [[ -z "$grub_lib" ]]; then
        log_warn "GRUB library directory not found, will use grub-mkrescue fallback"
        return 0
    fi

    log_info "Using GRUB from: $grub_lib"

    # Copy BIOS boot modules
    if [[ -d "$grub_lib/i386-pc" ]]; then
        # Create BIOS boot image
        local bios_modules="biosdisk iso9660 part_msdos part_gpt fat ext2 normal search configfile linux boot minicmd"

        if command -v grub-mkimage &> /dev/null; then
            grub-mkimage -O i386-pc -o "$ISO_DIR/boot/grub/i386-pc/core.img" \
                -p /boot/grub $bios_modules 2>/dev/null || true
        fi

        # Copy essential GRUB files for BIOS
        cp "$grub_lib/i386-pc/"*.mod "$ISO_DIR/boot/grub/i386-pc/" 2>/dev/null || true
        cp "$grub_lib/i386-pc/"*.lst "$ISO_DIR/boot/grub/i386-pc/" 2>/dev/null || true

        # Create eltorito boot image
        if [[ -f "$grub_lib/i386-pc/cdboot.img" && -f "$ISO_DIR/boot/grub/i386-pc/core.img" ]]; then
            cat "$grub_lib/i386-pc/cdboot.img" "$ISO_DIR/boot/grub/i386-pc/core.img" \
                > "$ISO_DIR/boot/grub/i386-pc/eltorito.img"
            log_info "Created BIOS eltorito boot image"
        fi
    fi

    # Copy EFI boot files
    local efi_arch=""
    local efi_name=""
    case "$ARCH" in
        x86_64)
            efi_arch="x86_64-efi"
            efi_name="BOOTX64.EFI"
            ;;
        aarch64)
            efi_arch="arm64-efi"
            efi_name="BOOTAA64.EFI"
            ;;
        riscv64)
            efi_arch="riscv64-efi"
            efi_name="BOOTRISCV64.EFI"
            ;;
    esac

    if [[ -d "$grub_lib/$efi_arch" ]]; then
        # Create EFI GRUB image
        local efi_modules="part_gpt part_msdos fat iso9660 normal search configfile linux boot minicmd"

        if command -v grub-mkimage &> /dev/null; then
            grub-mkimage -O "$efi_arch" -o "$ISO_DIR/EFI/BOOT/$efi_name" \
                -p /boot/grub $efi_modules 2>/dev/null || true
        fi

        # Copy EFI modules
        cp "$grub_lib/$efi_arch/"*.mod "$ISO_DIR/boot/grub/$efi_arch/" 2>/dev/null || true
        cp "$grub_lib/$efi_arch/"*.lst "$ISO_DIR/boot/grub/$efi_arch/" 2>/dev/null || true

        # Create EFI boot image (FAT filesystem)
        if [[ -f "$ISO_DIR/EFI/BOOT/$efi_name" ]]; then
            local efi_size=4096  # 4MB should be enough
            dd if=/dev/zero of="$ISO_DIR/EFI/efiboot.img" bs=1K count=$efi_size 2>/dev/null
            mkfs.vfat "$ISO_DIR/EFI/efiboot.img" 2>/dev/null || mkfs.fat "$ISO_DIR/EFI/efiboot.img" 2>/dev/null || true

            local efi_mount="$WORK_DIR/efi_mount"
            mkdir -p "$efi_mount"

            if mount -o loop "$ISO_DIR/EFI/efiboot.img" "$efi_mount" 2>/dev/null; then
                mkdir -p "$efi_mount/EFI/BOOT"
                cp "$ISO_DIR/EFI/BOOT/$efi_name" "$efi_mount/EFI/BOOT/"
                cp "$ISO_DIR/boot/grub/grub.cfg" "$efi_mount/EFI/BOOT/" 2>/dev/null || true
                umount "$efi_mount"
                log_info "Created EFI boot image"
            else
                log_warn "Could not mount EFI image (needs root), skipping EFI boot setup"
                rm -f "$ISO_DIR/EFI/efiboot.img"
            fi
            rmdir "$efi_mount" 2>/dev/null || true
        fi
    fi

    log_success "Bootloader setup complete"
}

# Create ISO structure
create_iso_structure() {
    log_info "Creating ISO structure..."

    mkdir -p "$ISO_DIR"/{boot/grub,EFI/BOOT,isolinux}

    # Create squashfs from rootfs
    mksquashfs "$ROOTFS_DIR" "$ISO_DIR/horcrux.squashfs" \
        -comp xz -b 1M -Xdict-size 100%

    # Copy kernel
    if [[ -f "$WORK_DIR/vmlinuz" ]]; then
        cp "$WORK_DIR/vmlinuz" "$ISO_DIR/boot/vmlinuz"
        log_info "Copied kernel to ISO"
    else
        log_error "Kernel not found in work directory!"
        exit 1
    fi

    # Copy initramfs
    cp "$WORK_DIR/initramfs.cpio.gz" "$ISO_DIR/boot/"

    log_success "ISO structure created"
}

# Create GRUB configuration
create_grub_config() {
    log_info "Creating GRUB configuration..."

    cat > "$ISO_DIR/boot/grub/grub.cfg" << EOF
set timeout=10
set default=0

menuentry "Horcrux $VERSION ($ARCH)" {
    linux /boot/vmlinuz root=/dev/sr0 init=/sbin/init quiet
    initrd /boot/initramfs.cpio.gz
}

menuentry "Horcrux $VERSION ($ARCH) - Debug Mode" {
    linux /boot/vmlinuz root=/dev/sr0 init=/sbin/init debug
    initrd /boot/initramfs.cpio.gz
}

menuentry "Horcrux $VERSION ($ARCH) - Rescue Shell" {
    linux /boot/vmlinuz root=/dev/sr0 init=/bin/sh
    initrd /boot/initramfs.cpio.gz
}
EOF

    log_success "GRUB configuration created"
}

# Build the ISO
build_iso() {
    log_info "Building ISO image..."

    mkdir -p "$OUTPUT_DIR"

    local iso_created=false

    # Try grub-mkrescue first (best option - handles BIOS and EFI automatically)
    if command -v grub-mkrescue &> /dev/null; then
        log_info "Using grub-mkrescue for ISO creation (supports BIOS and EFI)..."
        if grub-mkrescue -o "$OUTPUT_DIR/$ISO_NAME" "$ISO_DIR" \
            --product-name="Horcrux" \
            --product-version="$VERSION" \
            -- -volid "HORCRUX_${VERSION}" 2>&1 | tee /tmp/grub-mkrescue.log; then
            iso_created=true
            log_success "ISO created with grub-mkrescue"
        else
            log_warn "grub-mkrescue failed, trying alternatives..."
            cat /tmp/grub-mkrescue.log || true
        fi
    fi

    # Try xorriso with proper bootloader files if available
    if [[ "$iso_created" != "true" ]] && command -v xorriso &> /dev/null; then
        # Check if we have the required bootloader files
        if [[ -f "$ISO_DIR/boot/grub/i386-pc/eltorito.img" ]]; then
            log_info "Using xorriso with BIOS boot..."
            xorriso -as mkisofs \
                -iso-level 3 \
                -full-iso9660-filenames \
                -volid "HORCRUX_${VERSION}" \
                -eltorito-boot boot/grub/i386-pc/eltorito.img \
                -no-emul-boot \
                -boot-load-size 4 \
                -boot-info-table \
                --eltorito-catalog boot/grub/boot.cat \
                --grub2-boot-info \
                --grub2-mbr /usr/lib/grub/i386-pc/boot_hybrid.img \
                -eltorito-alt-boot \
                -e EFI/efiboot.img \
                -no-emul-boot \
                -append_partition 2 0xef "$ISO_DIR/EFI/efiboot.img" \
                -output "$OUTPUT_DIR/$ISO_NAME" \
                "$ISO_DIR" 2>/dev/null && iso_created=true
        elif [[ -f "$ISO_DIR/EFI/efiboot.img" ]]; then
            log_info "Using xorriso with EFI-only boot..."
            xorriso -as mkisofs \
                -iso-level 3 \
                -full-iso9660-filenames \
                -volid "HORCRUX_${VERSION}" \
                -eltorito-alt-boot \
                -e EFI/efiboot.img \
                -no-emul-boot \
                -output "$OUTPUT_DIR/$ISO_NAME" \
                "$ISO_DIR" 2>/dev/null && iso_created=true
        fi

        # Fallback to data-only ISO (won't boot but preserves files)
        if [[ "$iso_created" != "true" ]]; then
            log_warn "Creating data-only ISO (may not boot without external bootloader)..."
            xorriso -as mkisofs \
                -R -J \
                -volid "HORCRUX_${VERSION}" \
                -output "$OUTPUT_DIR/$ISO_NAME" \
                "$ISO_DIR" && iso_created=true
        fi
    fi

    # Fallback to mkisofs/genisoimage
    if [[ "$iso_created" != "true" ]]; then
        local mkiso_cmd=""
        if command -v mkisofs &> /dev/null; then
            mkiso_cmd="mkisofs"
        elif command -v genisoimage &> /dev/null; then
            mkiso_cmd="genisoimage"
        fi

        if [[ -n "$mkiso_cmd" ]]; then
            log_info "Using $mkiso_cmd for ISO creation..."
            $mkiso_cmd \
                -R -J -T \
                -V "HORCRUX_${VERSION}" \
                -b boot/grub/i386-pc/eltorito.img \
                -c boot/grub/boot.cat \
                -no-emul-boot \
                -boot-load-size 4 \
                -boot-info-table \
                -o "$OUTPUT_DIR/$ISO_NAME" \
                "$ISO_DIR" 2>/dev/null && iso_created=true

            # Simpler fallback
            if [[ "$iso_created" != "true" ]]; then
                log_warn "Falling back to basic $mkiso_cmd ISO creation..."
                $mkiso_cmd \
                    -R -J \
                    -V "HORCRUX_${VERSION}" \
                    -o "$OUTPUT_DIR/$ISO_NAME" \
                    "$ISO_DIR" && iso_created=true
            fi
        fi
    fi

    if [[ "$iso_created" != "true" ]]; then
        log_error "Failed to create ISO image"
        log_error "Please ensure grub-mkrescue or xorriso is installed"
        exit 1
    fi

    # Generate checksums
    cd "$OUTPUT_DIR"
    sha256sum "$ISO_NAME" > "${ISO_NAME}.sha256"
    sha512sum "$ISO_NAME" > "${ISO_NAME}.sha512"

    log_success "ISO built: $OUTPUT_DIR/$ISO_NAME"
}

# Print summary
print_summary() {
    local iso_size=$(du -h "$OUTPUT_DIR/$ISO_NAME" | cut -f1)

    echo ""
    log_info "=============================================="
    log_success "Build Complete!"
    log_info "=============================================="
    log_info "ISO:      $OUTPUT_DIR/$ISO_NAME"
    log_info "Size:     $iso_size"
    log_info "SHA256:   $(cat "$OUTPUT_DIR/${ISO_NAME}.sha256" | cut -d' ' -f1)"
    log_info ""
    log_info "Test with QEMU:"
    log_info "  $QEMU_SYSTEM -cdrom $OUTPUT_DIR/$ISO_NAME -m 2G"
    log_info "=============================================="
}

# Main build process
main() {
    check_requirements
    clean_build
    setup_rust_target
    setup_kernel
    build_binaries
    create_rootfs
    create_configs
    create_initramfs
    create_iso_structure
    create_grub_config
    setup_bootloader
    build_iso
    print_summary
}

main "$@"
