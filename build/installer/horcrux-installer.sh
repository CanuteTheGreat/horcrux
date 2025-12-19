#!/bin/bash
#
# Horcrux Installer
# Interactive installer for Horcrux Virtualization Platform
#
# This script installs Horcrux to a target disk, configuring:
#   - Disk partitioning (GPT with EFI or BIOS boot)
#   - Filesystem creation (ext4, XFS, or ZFS)
#   - Base system installation from squashfs
#   - Bootloader installation (GRUB)
#   - Network configuration
#   - User account setup
#   - Horcrux service configuration
#
# Usage: horcrux-installer [OPTIONS]
#
# Options:
#   -d, --disk DISK       Target disk (e.g., /dev/sda)
#   -a, --auto            Automatic installation with defaults
#   -c, --config FILE     Use configuration file
#   -h, --help            Show this help
#
# The installer can run in interactive mode (default) or automatic mode.

set -euo pipefail

# ============================================================================
# CONSTANTS AND DEFAULTS
# ============================================================================

VERSION="0.1.0"
INSTALLER_NAME="Horcrux Installer"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
WHITE='\033[1;37m'
NC='\033[0m'
BOLD='\033[1m'

# Installation source (set by ISO boot)
SQUASHFS_PATH="${SQUASHFS_PATH:-/cdrom/rootfs.squashfs}"
LIVE_ROOT="${LIVE_ROOT:-/}"

# Target mount point
TARGET="/mnt/target"

# Default values
DEFAULT_HOSTNAME="horcrux"
DEFAULT_TIMEZONE="UTC"
DEFAULT_LOCALE="en_US.UTF-8"
DEFAULT_KEYMAP="us"
DEFAULT_FS="ext4"
DEFAULT_SWAP_SIZE="4G"

# Installation state
INSTALL_DISK=""
INSTALL_MODE="interactive"
CONFIG_FILE=""
EFI_BOOT=false
USE_ZFS=false
DISK_LAYOUT=""

# ============================================================================
# UTILITY FUNCTIONS
# ============================================================================

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
log_step()    { echo -e "${CYAN}[STEP]${NC} ${BOLD}$1${NC}"; }

die() {
    log_error "$1"
    exit 1
}

confirm() {
    local prompt="$1"
    local default="${2:-n}"
    local response

    if [[ "$default" == "y" ]]; then
        prompt="$prompt [Y/n] "
    else
        prompt="$prompt [y/N] "
    fi

    read -r -p "$prompt" response
    response="${response,,}"  # to lowercase

    if [[ -z "$response" ]]; then
        response="$default"
    fi

    [[ "$response" == "y" || "$response" == "yes" ]]
}

press_enter() {
    read -r -p "Press Enter to continue..."
}

# ============================================================================
# UI FUNCTIONS
# ============================================================================

clear_screen() {
    clear
}

show_header() {
    clear_screen
    echo -e "${CYAN}"
    cat << 'EOF'
  _    _
 | |  | |
 | |__| | ___  _ __ ___ _ __ _   ___  __
 |  __  |/ _ \| '__/ __| '__| | | \ \/ /
 | |  | | (_) | | | (__| |  | |_| |>  <
 |_|  |_|\___/|_|  \___|_|   \__,_/_/\_\

EOF
    echo -e "${NC}"
    echo -e "${WHITE}${BOLD}Gentoo Virtualization Platform - Installer${NC}"
    echo -e "${BLUE}Version: $VERSION${NC}"
    echo ""
}

show_menu() {
    local title="$1"
    shift
    local options=("$@")

    echo -e "${WHITE}${BOLD}$title${NC}"
    echo ""

    local i=1
    for opt in "${options[@]}"; do
        echo -e "  ${CYAN}$i)${NC} $opt"
        ((i++))
    done
    echo ""
}

get_choice() {
    local max="$1"
    local prompt="${2:-Choice}"
    local choice

    while true; do
        read -r -p "$prompt [1-$max]: " choice
        if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "$max" ]; then
            echo "$choice"
            return
        fi
        log_error "Invalid choice. Please enter a number between 1 and $max."
    done
}

get_input() {
    local prompt="$1"
    local default="${2:-}"
    local value

    if [[ -n "$default" ]]; then
        read -r -p "$prompt [$default]: " value
        echo "${value:-$default}"
    else
        read -r -p "$prompt: " value
        echo "$value"
    fi
}

get_password() {
    local prompt="$1"
    local pass1 pass2

    while true; do
        read -r -s -p "$prompt: " pass1
        echo ""
        read -r -s -p "Confirm password: " pass2
        echo ""

        if [[ "$pass1" == "$pass2" ]]; then
            if [[ ${#pass1} -lt 4 ]]; then
                log_error "Password must be at least 4 characters"
                continue
            fi
            echo "$pass1"
            return
        fi
        log_error "Passwords do not match. Please try again."
    done
}

# ============================================================================
# SYSTEM DETECTION
# ============================================================================

detect_system() {
    log_step "Detecting system configuration..."

    # Check for EFI
    if [[ -d /sys/firmware/efi ]]; then
        EFI_BOOT=true
        log_info "EFI boot detected"
    else
        EFI_BOOT=false
        log_info "BIOS/Legacy boot detected"
    fi

    # Check available memory
    local mem_kb=$(grep MemTotal /proc/meminfo | awk '{print $2}')
    local mem_gb=$((mem_kb / 1024 / 1024))
    log_info "Available memory: ${mem_gb}GB"

    # Check CPU
    local cpu_count=$(nproc)
    local cpu_model=$(grep "model name" /proc/cpuinfo | head -1 | cut -d: -f2 | xargs)
    log_info "CPU: $cpu_model ($cpu_count cores)"

    # Check for virtualization support
    if grep -qE '(vmx|svm)' /proc/cpuinfo; then
        log_info "Hardware virtualization: Supported"
    else
        log_warn "Hardware virtualization: Not detected (VMs will be slow)"
    fi

    echo ""
}

list_disks() {
    log_step "Available disks:"
    echo ""

    lsblk -d -o NAME,SIZE,MODEL,TYPE | grep -E "^(NAME|[a-z]+.*disk)" | while read -r line; do
        if [[ "$line" == NAME* ]]; then
            printf "  %-10s %-10s %-30s %s\n" "DEVICE" "SIZE" "MODEL" "TYPE"
            echo "  --------------------------------------------------------"
        else
            local name=$(echo "$line" | awk '{print $1}')
            local size=$(echo "$line" | awk '{print $2}')
            local model=$(echo "$line" | awk '{$1=$2=$NF=""; print $0}' | xargs)
            local type=$(echo "$line" | awk '{print $NF}')
            printf "  %-10s %-10s %-30s %s\n" "/dev/$name" "$size" "$model" "$type"
        fi
    done
    echo ""
}

# ============================================================================
# DISK PARTITIONING
# ============================================================================

select_disk() {
    log_step "Disk Selection"

    list_disks

    while true; do
        INSTALL_DISK=$(get_input "Enter target disk (e.g., /dev/sda)")

        if [[ ! -b "$INSTALL_DISK" ]]; then
            log_error "Device $INSTALL_DISK does not exist"
            continue
        fi

        # Confirm disk selection
        echo ""
        log_warn "WARNING: All data on $INSTALL_DISK will be DESTROYED!"
        echo ""
        lsblk "$INSTALL_DISK"
        echo ""

        if confirm "Are you sure you want to use $INSTALL_DISK?" "n"; then
            break
        fi
    done
}

select_filesystem() {
    log_step "Filesystem Selection"
    echo ""

    show_menu "Select root filesystem:" \
        "ext4 - Standard Linux filesystem (recommended)" \
        "XFS - High-performance filesystem" \
        "ZFS - Advanced filesystem with snapshots (requires more RAM)" \
        "Btrfs - Copy-on-write filesystem with snapshots"

    local choice=$(get_choice 4)

    case $choice in
        1) DEFAULT_FS="ext4" ;;
        2) DEFAULT_FS="xfs" ;;
        3) DEFAULT_FS="zfs"; USE_ZFS=true ;;
        4) DEFAULT_FS="btrfs" ;;
    esac

    log_info "Selected filesystem: $DEFAULT_FS"
}

select_disk_layout() {
    log_step "Disk Layout"
    echo ""

    show_menu "Select disk layout:" \
        "Automatic - Single disk, standard partitions (recommended)" \
        "LVM - Logical Volume Manager (flexible resizing)" \
        "ZFS - ZFS root with boot partition" \
        "Manual - Configure partitions manually"

    local choice=$(get_choice 4)

    case $choice in
        1) DISK_LAYOUT="standard" ;;
        2) DISK_LAYOUT="lvm" ;;
        3) DISK_LAYOUT="zfs"; USE_ZFS=true ;;
        4) DISK_LAYOUT="manual" ;;
    esac

    log_info "Selected layout: $DISK_LAYOUT"
}

partition_disk_standard() {
    log_step "Creating standard partition layout on $INSTALL_DISK..."

    # Wipe existing partition table
    wipefs -a "$INSTALL_DISK" >/dev/null 2>&1 || true
    sgdisk -Z "$INSTALL_DISK" >/dev/null 2>&1 || true

    if $EFI_BOOT; then
        # GPT with EFI
        log_info "Creating GPT partition table with EFI..."

        sgdisk -n 1:0:+512M -t 1:ef00 -c 1:"EFI System" "$INSTALL_DISK"
        sgdisk -n 2:0:+1G -t 2:8300 -c 2:"Boot" "$INSTALL_DISK"
        sgdisk -n 3:0:+${DEFAULT_SWAP_SIZE} -t 3:8200 -c 3:"Swap" "$INSTALL_DISK"
        sgdisk -n 4:0:0 -t 4:8300 -c 4:"Root" "$INSTALL_DISK"

        # Partition names
        PART_EFI="${INSTALL_DISK}1"
        PART_BOOT="${INSTALL_DISK}2"
        PART_SWAP="${INSTALL_DISK}3"
        PART_ROOT="${INSTALL_DISK}4"

        # Handle nvme naming
        if [[ "$INSTALL_DISK" == *"nvme"* ]]; then
            PART_EFI="${INSTALL_DISK}p1"
            PART_BOOT="${INSTALL_DISK}p2"
            PART_SWAP="${INSTALL_DISK}p3"
            PART_ROOT="${INSTALL_DISK}p4"
        fi
    else
        # GPT with BIOS boot
        log_info "Creating GPT partition table with BIOS boot..."

        sgdisk -n 1:0:+1M -t 1:ef02 -c 1:"BIOS Boot" "$INSTALL_DISK"
        sgdisk -n 2:0:+1G -t 2:8300 -c 2:"Boot" "$INSTALL_DISK"
        sgdisk -n 3:0:+${DEFAULT_SWAP_SIZE} -t 3:8200 -c 3:"Swap" "$INSTALL_DISK"
        sgdisk -n 4:0:0 -t 4:8300 -c 4:"Root" "$INSTALL_DISK"

        PART_BIOS="${INSTALL_DISK}1"
        PART_BOOT="${INSTALL_DISK}2"
        PART_SWAP="${INSTALL_DISK}3"
        PART_ROOT="${INSTALL_DISK}4"

        if [[ "$INSTALL_DISK" == *"nvme"* ]]; then
            PART_BIOS="${INSTALL_DISK}p1"
            PART_BOOT="${INSTALL_DISK}p2"
            PART_SWAP="${INSTALL_DISK}p3"
            PART_ROOT="${INSTALL_DISK}p4"
        fi
    fi

    # Wait for partitions to appear
    sleep 2
    partprobe "$INSTALL_DISK" 2>/dev/null || true
    sleep 1

    log_success "Partitions created"
}

format_partitions() {
    log_step "Formatting partitions..."

    if $EFI_BOOT; then
        log_info "Formatting EFI partition..."
        mkfs.vfat -F32 "$PART_EFI"
    fi

    log_info "Formatting boot partition..."
    mkfs.ext4 -F -L boot "$PART_BOOT"

    log_info "Creating swap..."
    mkswap -L swap "$PART_SWAP"

    log_info "Formatting root partition ($DEFAULT_FS)..."
    case $DEFAULT_FS in
        ext4)
            mkfs.ext4 -F -L root "$PART_ROOT"
            ;;
        xfs)
            mkfs.xfs -f -L root "$PART_ROOT"
            ;;
        btrfs)
            mkfs.btrfs -f -L root "$PART_ROOT"
            ;;
    esac

    log_success "Partitions formatted"
}

mount_partitions() {
    log_step "Mounting partitions..."

    # Create and mount root
    mkdir -p "$TARGET"
    mount "$PART_ROOT" "$TARGET"

    # Create mount points
    mkdir -p "$TARGET/boot"
    mount "$PART_BOOT" "$TARGET/boot"

    if $EFI_BOOT; then
        mkdir -p "$TARGET/boot/efi"
        mount "$PART_EFI" "$TARGET/boot/efi"
    fi

    # Enable swap
    swapon "$PART_SWAP"

    log_success "Partitions mounted at $TARGET"
}

# ============================================================================
# SYSTEM INSTALLATION
# ============================================================================

install_base_system() {
    log_step "Installing base system..."

    if [[ ! -f "$SQUASHFS_PATH" ]]; then
        # Try to find squashfs
        for path in /cdrom/rootfs.squashfs /mnt/cdrom/rootfs.squashfs /run/initramfs/live/rootfs.squashfs; do
            if [[ -f "$path" ]]; then
                SQUASHFS_PATH="$path"
                break
            fi
        done
    fi

    if [[ -f "$SQUASHFS_PATH" ]]; then
        log_info "Installing from squashfs: $SQUASHFS_PATH"
        unsquashfs -f -d "$TARGET" "$SQUASHFS_PATH"
    else
        log_info "Copying live system to target..."
        rsync -aAXv --exclude={"/dev/*","/proc/*","/sys/*","/tmp/*","/run/*","/mnt/*","/media/*","/lost+found","/cdrom/*"} \
            "$LIVE_ROOT" "$TARGET/"
    fi

    log_success "Base system installed"
}

configure_fstab() {
    log_step "Configuring fstab..."

    local root_uuid=$(blkid -s UUID -o value "$PART_ROOT")
    local boot_uuid=$(blkid -s UUID -o value "$PART_BOOT")
    local swap_uuid=$(blkid -s UUID -o value "$PART_SWAP")

    cat > "$TARGET/etc/fstab" << EOF
# /etc/fstab - Horcrux installation
# <file system>  <mount point>  <type>  <options>  <dump>  <pass>

# Root filesystem
UUID=$root_uuid  /       $DEFAULT_FS  defaults,noatime  0  1

# Boot partition
UUID=$boot_uuid  /boot   ext4   defaults,noatime  0  2

EOF

    if $EFI_BOOT; then
        local efi_uuid=$(blkid -s UUID -o value "$PART_EFI")
        cat >> "$TARGET/etc/fstab" << EOF
# EFI System Partition
UUID=$efi_uuid   /boot/efi  vfat  umask=0077  0  2

EOF
    fi

    cat >> "$TARGET/etc/fstab" << EOF
# Swap
UUID=$swap_uuid  none    swap   sw  0  0

# Temporary filesystems
tmpfs  /tmp   tmpfs  defaults,nosuid,nodev  0  0
tmpfs  /run   tmpfs  defaults,nosuid,nodev,mode=755  0  0
EOF

    log_success "fstab configured"
}

# ============================================================================
# SYSTEM CONFIGURATION
# ============================================================================

configure_hostname() {
    local hostname=$(get_input "Enter hostname" "$DEFAULT_HOSTNAME")

    echo "$hostname" > "$TARGET/etc/hostname"

    cat > "$TARGET/etc/hosts" << EOF
127.0.0.1       localhost
::1             localhost
127.0.1.1       $hostname.localdomain $hostname
EOF

    log_success "Hostname set to: $hostname"
}

configure_timezone() {
    log_step "Timezone Configuration"
    echo ""

    # Common timezones
    show_menu "Select timezone:" \
        "UTC (Coordinated Universal Time)" \
        "America/New_York (Eastern US)" \
        "America/Chicago (Central US)" \
        "America/Denver (Mountain US)" \
        "America/Los_Angeles (Pacific US)" \
        "Europe/London (UK)" \
        "Europe/Berlin (Central Europe)" \
        "Asia/Tokyo (Japan)" \
        "Other (enter manually)"

    local choice=$(get_choice 9)
    local tz=""

    case $choice in
        1) tz="UTC" ;;
        2) tz="America/New_York" ;;
        3) tz="America/Chicago" ;;
        4) tz="America/Denver" ;;
        5) tz="America/Los_Angeles" ;;
        6) tz="Europe/London" ;;
        7) tz="Europe/Berlin" ;;
        8) tz="Asia/Tokyo" ;;
        9) tz=$(get_input "Enter timezone (e.g., America/New_York)") ;;
    esac

    ln -sf "/usr/share/zoneinfo/$tz" "$TARGET/etc/localtime"
    echo "$tz" > "$TARGET/etc/timezone"

    log_success "Timezone set to: $tz"
}

configure_locale() {
    log_step "Locale Configuration"

    local locale=$(get_input "Enter locale" "$DEFAULT_LOCALE")

    # Enable locale
    echo "$locale UTF-8" >> "$TARGET/etc/locale.gen"

    # Generate locale in chroot
    chroot "$TARGET" locale-gen 2>/dev/null || true

    # Set default locale
    echo "LANG=\"$locale\"" > "$TARGET/etc/locale.conf"

    log_success "Locale set to: $locale"
}

configure_root_password() {
    log_step "Root Password"
    echo ""
    log_info "Set the root password for your system."
    echo ""

    local password=$(get_password "Enter root password")

    echo "root:$password" | chroot "$TARGET" chpasswd

    log_success "Root password set"
}

configure_user() {
    log_step "User Account"
    echo ""

    if ! confirm "Create a regular user account?" "y"; then
        return
    fi

    local username=$(get_input "Enter username" "horcrux")
    local fullname=$(get_input "Enter full name" "Horcrux Administrator")
    local password=$(get_password "Enter password for $username")

    # Create user
    chroot "$TARGET" useradd -m -G wheel,kvm,libvirt,docker -c "$fullname" -s /bin/bash "$username" 2>/dev/null || true
    echo "$username:$password" | chroot "$TARGET" chpasswd

    # Configure sudo
    echo '%wheel ALL=(ALL) ALL' > "$TARGET/etc/sudoers.d/wheel"
    chmod 440 "$TARGET/etc/sudoers.d/wheel"

    log_success "User $username created"
}

configure_network() {
    log_step "Network Configuration"
    echo ""

    show_menu "Select network configuration:" \
        "DHCP (automatic, recommended)" \
        "Static IP" \
        "Skip (configure later)"

    local choice=$(get_choice 3)

    case $choice in
        1)
            # Enable DHCP service
            chroot "$TARGET" rc-update add dhcpcd default 2>/dev/null || true
            log_success "DHCP enabled"
            ;;
        2)
            local iface=$(get_input "Network interface" "eth0")
            local ip=$(get_input "IP address (e.g., 192.168.1.100/24)")
            local gateway=$(get_input "Gateway")
            local dns=$(get_input "DNS server" "8.8.8.8")

            # Configure static IP
            cat > "$TARGET/etc/conf.d/net" << EOF
# Static network configuration
config_$iface="$ip"
routes_$iface="default via $gateway"
dns_servers_$iface="$dns"
EOF

            # Create init script link
            ln -sf net.lo "$TARGET/etc/init.d/net.$iface" 2>/dev/null || true
            chroot "$TARGET" rc-update add "net.$iface" default 2>/dev/null || true

            log_success "Static IP configured: $ip"
            ;;
        3)
            log_info "Network configuration skipped"
            ;;
    esac
}

# ============================================================================
# BOOTLOADER INSTALLATION
# ============================================================================

install_bootloader() {
    log_step "Installing bootloader..."

    # Mount required filesystems for chroot
    mount --bind /dev "$TARGET/dev"
    mount --bind /dev/pts "$TARGET/dev/pts"
    mount --bind /proc "$TARGET/proc"
    mount --bind /sys "$TARGET/sys"

    if $EFI_BOOT; then
        log_info "Installing GRUB for EFI..."
        chroot "$TARGET" grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=Horcrux --removable 2>&1 || {
            log_warn "EFI install failed, trying without --removable..."
            chroot "$TARGET" grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=Horcrux 2>&1 || true
        }
    else
        log_info "Installing GRUB for BIOS..."
        chroot "$TARGET" grub-install "$INSTALL_DISK" 2>&1 || true
    fi

    # Generate GRUB config
    log_info "Generating GRUB configuration..."
    chroot "$TARGET" grub-mkconfig -o /boot/grub/grub.cfg 2>&1

    # Cleanup mounts
    umount "$TARGET/sys" 2>/dev/null || true
    umount "$TARGET/proc" 2>/dev/null || true
    umount "$TARGET/dev/pts" 2>/dev/null || true
    umount "$TARGET/dev" 2>/dev/null || true

    log_success "Bootloader installed"
}

# ============================================================================
# HORCRUX CONFIGURATION
# ============================================================================

configure_horcrux() {
    log_step "Configuring Horcrux..."

    # Ensure directories exist
    mkdir -p "$TARGET/etc/horcrux"
    mkdir -p "$TARGET/var/lib/horcrux"/{images,backups,templates}
    mkdir -p "$TARGET/var/log/horcrux"

    # Enable Horcrux service
    chroot "$TARGET" rc-update add horcrux default 2>/dev/null || true

    # Enable other required services
    chroot "$TARGET" rc-update add libvirtd default 2>/dev/null || true
    chroot "$TARGET" rc-update add docker default 2>/dev/null || true
    chroot "$TARGET" rc-update add sshd default 2>/dev/null || true
    chroot "$TARGET" rc-update add dbus default 2>/dev/null || true

    log_success "Horcrux configured"
}

# ============================================================================
# INSTALLATION SUMMARY
# ============================================================================

show_summary() {
    show_header
    log_step "Installation Summary"
    echo ""

    echo -e "  ${WHITE}Target Disk:${NC}     $INSTALL_DISK"
    echo -e "  ${WHITE}Boot Mode:${NC}       $(if $EFI_BOOT; then echo "EFI"; else echo "BIOS"; fi)"
    echo -e "  ${WHITE}Filesystem:${NC}      $DEFAULT_FS"
    echo -e "  ${WHITE}Disk Layout:${NC}     $DISK_LAYOUT"
    echo ""

    echo -e "  ${WHITE}Partitions:${NC}"
    lsblk "$INSTALL_DISK" -o NAME,SIZE,TYPE,MOUNTPOINT 2>/dev/null | sed 's/^/    /'
    echo ""
}

show_completion() {
    show_header
    echo -e "${GREEN}${BOLD}Installation Complete!${NC}"
    echo ""
    echo -e "Horcrux has been successfully installed to ${WHITE}$INSTALL_DISK${NC}"
    echo ""
    echo -e "${WHITE}Next Steps:${NC}"
    echo "  1. Remove the installation media"
    echo "  2. Reboot the system"
    echo "  3. Log in as root or your created user"
    echo "  4. Access the web UI at: http://<your-ip>:8006"
    echo ""
    echo -e "${WHITE}Default Services:${NC}"
    echo "  - Horcrux API:   http://localhost:8006"
    echo "  - SSH:           Port 22"
    echo "  - Libvirt:       Running"
    echo "  - Docker:        Running"
    echo ""
    echo -e "${WHITE}Useful Commands:${NC}"
    echo "  horcrux --help           # CLI help"
    echo "  horcrux vm list          # List VMs"
    echo "  horcrux container list   # List containers"
    echo "  rc-service horcrux status  # Check service status"
    echo ""

    if confirm "Reboot now?" "y"; then
        log_info "Rebooting..."
        umount -R "$TARGET" 2>/dev/null || true
        reboot
    else
        log_info "You can reboot manually when ready: reboot"
    fi
}

# ============================================================================
# MAIN INSTALLATION FLOW
# ============================================================================

run_interactive_install() {
    show_header

    echo -e "${WHITE}Welcome to the Horcrux Installer${NC}"
    echo ""
    echo "This installer will guide you through installing Horcrux"
    echo "Virtualization Platform on your system."
    echo ""
    log_warn "WARNING: This will erase data on the selected disk!"
    echo ""

    if ! confirm "Continue with installation?" "y"; then
        log_info "Installation cancelled."
        exit 0
    fi

    # System detection
    detect_system

    # Disk selection and partitioning
    select_disk
    select_disk_layout

    if [[ "$DISK_LAYOUT" != "zfs" ]]; then
        select_filesystem
    fi

    # Show summary before proceeding
    show_header
    log_step "Installation Plan"
    echo ""
    echo -e "  ${WHITE}Target Disk:${NC}     $INSTALL_DISK"
    echo -e "  ${WHITE}Disk Layout:${NC}     $DISK_LAYOUT"
    echo -e "  ${WHITE}Filesystem:${NC}      $DEFAULT_FS"
    echo -e "  ${WHITE}Boot Mode:${NC}       $(if $EFI_BOOT; then echo "EFI"; else echo "BIOS"; fi)"
    echo ""

    if ! confirm "Proceed with installation? THIS WILL ERASE $INSTALL_DISK" "n"; then
        log_info "Installation cancelled."
        exit 0
    fi

    # Perform installation
    echo ""
    log_step "Starting installation..."
    echo ""

    partition_disk_standard
    format_partitions
    mount_partitions
    install_base_system
    configure_fstab

    # System configuration
    show_header
    configure_hostname
    configure_timezone
    configure_locale
    configure_root_password
    configure_user
    configure_network

    # Finalize
    install_bootloader
    configure_horcrux

    # Complete
    show_completion
}

run_automatic_install() {
    log_info "Running automatic installation..."

    if [[ -z "$INSTALL_DISK" ]]; then
        die "Automatic install requires -d/--disk option"
    fi

    detect_system
    DISK_LAYOUT="standard"

    partition_disk_standard
    format_partitions
    mount_partitions
    install_base_system
    configure_fstab

    # Use defaults
    echo "$DEFAULT_HOSTNAME" > "$TARGET/etc/hostname"
    ln -sf "/usr/share/zoneinfo/$DEFAULT_TIMEZONE" "$TARGET/etc/localtime"
    echo "root:horcrux" | chroot "$TARGET" chpasswd

    install_bootloader
    configure_horcrux

    log_success "Automatic installation complete"
    log_info "Default root password: horcrux"
}

# ============================================================================
# ARGUMENT PARSING
# ============================================================================

show_help() {
    cat << EOF
$INSTALLER_NAME v$VERSION

Usage: $(basename "$0") [OPTIONS]

Options:
  -d, --disk DISK       Target disk for installation (e.g., /dev/sda)
  -a, --auto            Run automatic installation with defaults
  -c, --config FILE     Use configuration file for answers
  -h, --help            Show this help message

Examples:
  $(basename "$0")                    # Interactive installation
  $(basename "$0") -d /dev/sda -a     # Automatic install to /dev/sda
  $(basename "$0") -c install.conf    # Use config file

For more information, visit: https://github.com/horcrux/horcrux
EOF
    exit 0
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -d|--disk)
                INSTALL_DISK="$2"
                shift 2
                ;;
            -a|--auto)
                INSTALL_MODE="automatic"
                shift
                ;;
            -c|--config)
                CONFIG_FILE="$2"
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
}

# ============================================================================
# MAIN
# ============================================================================

main() {
    # Check root
    if [[ $EUID -ne 0 ]]; then
        die "This installer must be run as root"
    fi

    # Parse arguments
    parse_args "$@"

    # Check for required tools
    for cmd in sgdisk mkfs.ext4 mount chroot grub-install; do
        if ! command -v "$cmd" &>/dev/null; then
            die "Required command not found: $cmd"
        fi
    done

    # Run installation
    case $INSTALL_MODE in
        interactive)
            run_interactive_install
            ;;
        automatic)
            run_automatic_install
            ;;
    esac
}

main "$@"
