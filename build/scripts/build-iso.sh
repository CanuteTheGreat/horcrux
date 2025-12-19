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
        "cpio"
    )

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
#!/bin/busybox sh
# Horcrux initramfs init

# Mount essential filesystems
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev

# Parse kernel command line
cmdline=$(cat /proc/cmdline)
root=""
init="/sbin/init"

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

# Wait for root device
echo "Waiting for root device..."
sleep 2

# Mount root filesystem
if [ -n "$root" ]; then
    mount -o ro "$root" /newroot
else
    # Try to find squashfs
    if [ -e /dev/sr0 ]; then
        mount -t iso9660 -o ro /dev/sr0 /mnt
        mount -t squashfs -o loop /mnt/horcrux.squashfs /newroot
    fi
fi

# Switch to real root
umount /proc
umount /sys
umount /dev

exec switch_root /newroot "$init"
EOF
    chmod +x "$initramfs_dir/init"

    # Copy busybox (static) or individual tools
    if command -v busybox &> /dev/null; then
        cp "$(which busybox)" "$initramfs_dir/bin/"

        # Create symlinks for essential commands
        for cmd in sh mount umount switch_root cat sleep; do
            ln -sf /bin/busybox "$initramfs_dir/bin/$cmd"
        done
    else
        log_warn "busybox not found, copying individual tools..."
        # Copy essential binaries from system
        for cmd in sh bash mount umount cat sleep; do
            if command -v "$cmd" &> /dev/null; then
                cp "$(which $cmd)" "$initramfs_dir/bin/" 2>/dev/null || true
            fi
        done
        # Try to get switch_root from util-linux
        if [[ -f /sbin/switch_root ]]; then
            cp /sbin/switch_root "$initramfs_dir/sbin/"
        elif [[ -f /usr/sbin/switch_root ]]; then
            cp /usr/sbin/switch_root "$initramfs_dir/sbin/"
        fi
    fi

    # Create initramfs cpio archive
    (cd "$initramfs_dir" && find . | cpio -o -H newc | gzip > "$WORK_DIR/initramfs.cpio.gz")

    log_success "Initramfs created"
}

# Create ISO structure
create_iso_structure() {
    log_info "Creating ISO structure..."

    mkdir -p "$ISO_DIR"/{boot/grub,EFI/BOOT,isolinux}

    # Create squashfs from rootfs
    mksquashfs "$ROOTFS_DIR" "$ISO_DIR/horcrux.squashfs" \
        -comp xz -b 1M -Xdict-size 100%

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

    # Try xorriso first (best option for modern bootable ISOs)
    if command -v xorriso &> /dev/null; then
        log_info "Using xorriso for ISO creation..."
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

        # Fallback to simpler xorriso command
        if [[ "$iso_created" != "true" ]]; then
            log_warn "Falling back to basic xorriso ISO creation..."
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

    # Use grub-mkrescue as last resort
    if [[ "$iso_created" != "true" ]]; then
        log_warn "Falling back to grub-mkrescue..."
        grub-mkrescue -o "$OUTPUT_DIR/$ISO_NAME" "$ISO_DIR" && iso_created=true
    fi

    if [[ "$iso_created" != "true" ]]; then
        log_error "Failed to create ISO image"
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
    build_binaries
    create_rootfs
    create_configs
    create_initramfs
    create_iso_structure
    create_grub_config
    build_iso
    print_summary
}

main "$@"
