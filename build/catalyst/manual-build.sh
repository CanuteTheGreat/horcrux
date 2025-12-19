#!/bin/bash
#
# Horcrux Manual LiveCD Build Script
# A simpler alternative to Catalyst that uses direct chroot
#
# This script:
# 1. Extracts the stage3 tarball
# 2. Sets up proper chroot mounts
# 3. Installs packages
# 4. Creates the squashfs and ISO
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
SPECS_DIR="$SCRIPT_DIR/specs"
FILES_DIR="$SCRIPT_DIR/files"

# Catalyst directories (reuse stage3 and snapshot)
CATALYST_BASE="/var/tmp/catalyst"
STAGE3="$CATALYST_BASE/builds/horcrux/stage3-amd64-openrc-latest.tar.xz"
SNAPSHOT="$CATALYST_BASE/snapshots/gentoo-latest.sqfs"

# Build directories
WORK_DIR="$CATALYST_BASE/manual-build"
CHROOT_DIR="$WORK_DIR/chroot"
ISO_DIR="$WORK_DIR/iso"
OUTPUT_DIR="$PROJECT_ROOT/build/iso"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root"
        exit 1
    fi
}

cleanup() {
    log_info "Cleaning up mounts..."
    umount -R "$CHROOT_DIR/var/db/repos/gentoo" 2>/dev/null || true
    umount -R "$CHROOT_DIR/dev/pts" 2>/dev/null || true
    umount -R "$CHROOT_DIR/dev/shm" 2>/dev/null || true
    umount -R "$CHROOT_DIR/dev" 2>/dev/null || true
    umount -R "$CHROOT_DIR/proc" 2>/dev/null || true
    umount -R "$CHROOT_DIR/sys" 2>/dev/null || true
    umount -R "$CHROOT_DIR/run" 2>/dev/null || true
    umount "$CHROOT_DIR/var/db/repos/gentoo" 2>/dev/null || true
}

trap cleanup EXIT

setup_chroot() {
    log_info "Setting up chroot environment..."

    mkdir -p "$WORK_DIR" "$CHROOT_DIR" "$ISO_DIR" "$OUTPUT_DIR"

    # Extract stage3 if not already done
    if [[ ! -f "$CHROOT_DIR/etc/gentoo-release" ]]; then
        log_info "Extracting stage3 tarball..."
        tar xpf "$STAGE3" -C "$CHROOT_DIR" --xattrs-include='*.*' --numeric-owner
    else
        log_info "Stage3 already extracted"
    fi

    # Mount essential filesystems
    log_info "Mounting filesystems..."
    mount --bind /dev "$CHROOT_DIR/dev"
    mount --bind /dev/pts "$CHROOT_DIR/dev/pts"
    mount -t tmpfs tmpfs "$CHROOT_DIR/dev/shm"
    mount -t proc proc "$CHROOT_DIR/proc"
    mount --bind /sys "$CHROOT_DIR/sys"
    mount -t tmpfs tmpfs "$CHROOT_DIR/run"

    # Mount portage tree from squashfs
    mkdir -p "$CHROOT_DIR/var/db/repos/gentoo"
    mount -t squashfs -o loop,ro "$SNAPSHOT" "$CHROOT_DIR/var/db/repos/gentoo"

    # Copy resolv.conf for network access
    cp -L /etc/resolv.conf "$CHROOT_DIR/etc/resolv.conf"

    log_success "Chroot environment ready"
}

run_in_chroot() {
    chroot "$CHROOT_DIR" /bin/bash -c "$1"
}

configure_portage() {
    log_info "Configuring Portage..."

    # Copy portage configuration
    if [[ -d "$FILES_DIR/portage" ]]; then
        cp -a "$FILES_DIR/portage"/* "$CHROOT_DIR/etc/portage/" 2>/dev/null || true
    fi

    # Set profile
    run_in_chroot "eselect profile set default/linux/amd64/23.0" || true

    # Generate locale
    echo 'en_US.UTF-8 UTF-8' > "$CHROOT_DIR/etc/locale.gen"
    run_in_chroot "locale-gen" || true
    run_in_chroot "eselect locale set en_US.utf8" || true

    log_success "Portage configured"
}

install_packages() {
    log_info "Installing packages..."

    # Essential packages list
    local packages=(
        app-admin/sudo
        app-admin/sysklogd
        app-misc/screen
        app-misc/tmux
        app-editors/vim
        app-editors/nano
        dev-vcs/git
        net-misc/curl
        net-misc/wget
        net-misc/openssh
        net-misc/dhcpcd
        sys-apps/busybox
        sys-apps/pciutils
        sys-apps/usbutils
        sys-block/parted
        sys-fs/dosfstools
        sys-fs/e2fsprogs
        sys-fs/xfsprogs
        sys-fs/btrfs-progs
        sys-fs/lvm2
        sys-fs/squashfs-tools
        sys-process/htop
        sys-boot/grub
        sys-kernel/gentoo-kernel-bin
        sys-kernel/linux-firmware
    )

    # Install packages
    run_in_chroot "emerge --verbose --quiet-build --noreplace ${packages[*]}" || {
        log_warn "Some packages may have failed, continuing..."
    }

    log_success "Packages installed"
}

configure_system() {
    log_info "Configuring system..."

    # Set hostname
    echo "horcrux" > "$CHROOT_DIR/etc/hostname"

    # Configure hosts
    cat > "$CHROOT_DIR/etc/hosts" << 'EOF'
127.0.0.1   localhost horcrux
::1         localhost horcrux
EOF

    # Set timezone
    run_in_chroot "ln -sf /usr/share/zoneinfo/UTC /etc/localtime"

    # Set root password
    run_in_chroot "echo 'root:horcrux' | chpasswd"

    # Create horcrux user
    run_in_chroot "useradd -m -G wheel -s /bin/bash horcrux" || true
    run_in_chroot "echo 'horcrux:horcrux' | chpasswd"

    # Configure sudo
    mkdir -p "$CHROOT_DIR/etc/sudoers.d"
    echo '%wheel ALL=(ALL) ALL' > "$CHROOT_DIR/etc/sudoers.d/wheel"
    chmod 440 "$CHROOT_DIR/etc/sudoers.d/wheel"

    # Enable services
    run_in_chroot "rc-update add sshd default" || true
    run_in_chroot "rc-update add dhcpcd default" || true

    # Create motd
    cat > "$CHROOT_DIR/etc/motd" << 'EOF'

  _    _
 | |  | |
 | |__| | ___  _ __ ___ _ __ _   ___  __
 |  __  |/ _ \| '__/ __| '__| | | \ \/ /
 | |  | | (_) | | | (__| |  | |_| |>  <
 |_|  |_|\___/|_|  \___|_|   \__,_/_/\_\

 Gentoo Virtualization Platform

 Default credentials:
   Username: root / horcrux
   Password: horcrux

 To install: horcrux-installer

EOF

    log_success "System configured"
}

create_iso() {
    log_info "Creating ISO..."

    # Create ISO structure
    mkdir -p "$ISO_DIR"/{boot/grub,LiveOS}

    # Find kernel and initramfs
    local kernel=$(ls "$CHROOT_DIR/boot"/vmlinuz-* 2>/dev/null | head -1)
    local initrd=$(ls "$CHROOT_DIR/boot"/initramfs-* 2>/dev/null | head -1)

    if [[ -z "$kernel" ]]; then
        log_error "No kernel found in $CHROOT_DIR/boot/"
        ls -la "$CHROOT_DIR/boot/" || true
        exit 1
    fi

    log_info "Using kernel: $kernel"
    cp "$kernel" "$ISO_DIR/boot/vmlinuz"

    if [[ -n "$initrd" ]]; then
        log_info "Using initramfs: $initrd"
        cp "$initrd" "$ISO_DIR/boot/initramfs"
    fi

    # Create squashfs of the root filesystem
    log_info "Creating squashfs..."
    mksquashfs "$CHROOT_DIR" "$ISO_DIR/LiveOS/squashfs.img" \
        -comp zstd \
        -Xcompression-level 15 \
        -e "$CHROOT_DIR/var/tmp/*" \
        -e "$CHROOT_DIR/var/cache/distfiles/*" \
        -e "$CHROOT_DIR/usr/src/*" \
        -e "$CHROOT_DIR/var/db/repos/gentoo/*" \
        -progress

    # Create GRUB config
    cat > "$ISO_DIR/boot/grub/grub.cfg" << 'EOF'
set timeout=10
set default=0

menuentry "Horcrux (Live)" {
    linux /boot/vmlinuz root=live:CDLABEL=HORCRUX rd.live.image rd.live.dir=/LiveOS rd.live.squashimg=squashfs.img
    initrd /boot/initramfs
}

menuentry "Horcrux (Live - nomodeset)" {
    linux /boot/vmlinuz root=live:CDLABEL=HORCRUX rd.live.image rd.live.dir=/LiveOS rd.live.squashimg=squashfs.img nomodeset
    initrd /boot/initramfs
}

menuentry "Horcrux Installer" {
    linux /boot/vmlinuz root=live:CDLABEL=HORCRUX rd.live.image rd.live.dir=/LiveOS rd.live.squashimg=squashfs.img horcrux.mode=installer
    initrd /boot/initramfs
}
EOF

    # Create the ISO
    local iso_file="$OUTPUT_DIR/horcrux-$(date +%Y%m%d)-amd64.iso"

    log_info "Creating bootable ISO..."
    grub-mkrescue -o "$iso_file" "$ISO_DIR" \
        -volid "HORCRUX" \
        -- -volset "HORCRUX" 2>&1 || {
        log_warn "grub-mkrescue failed, trying xorriso..."
        xorriso -as mkisofs \
            -o "$iso_file" \
            -isohybrid-mbr /usr/share/grub/i386-pc/boot_hybrid.img \
            -c boot.cat \
            -b boot/grub/i386-pc/eltorito.img \
            -no-emul-boot \
            -boot-load-size 4 \
            -boot-info-table \
            --grub2-boot-info \
            -eltorito-alt-boot \
            -e boot/grub/efi.img \
            -no-emul-boot \
            -isohybrid-gpt-basdat \
            -V "HORCRUX" \
            "$ISO_DIR"
    }

    if [[ -f "$iso_file" ]]; then
        local size=$(stat -c%s "$iso_file")
        log_success "ISO created: $iso_file ($((size / 1024 / 1024)) MB)"

        # Generate checksums
        cd "$OUTPUT_DIR"
        sha256sum "$(basename "$iso_file")" > "$(basename "$iso_file").sha256"
        log_success "Checksum: $(cat "$(basename "$iso_file").sha256")"
    else
        log_error "Failed to create ISO"
        exit 1
    fi
}

main() {
    check_root

    log_info "Horcrux Manual LiveCD Build"
    log_info "============================"

    # Check prerequisites
    if [[ ! -f "$STAGE3" ]]; then
        log_error "Stage3 not found: $STAGE3"
        log_info "Run: sudo ./build.sh --fetch-seed"
        exit 1
    fi

    if [[ ! -f "$SNAPSHOT" ]]; then
        log_error "Portage snapshot not found: $SNAPSHOT"
        log_info "Run: sudo ./build.sh --snapshot"
        exit 1
    fi

    setup_chroot
    configure_portage
    install_packages
    configure_system
    create_iso

    log_success "Build complete!"
}

main "$@"
