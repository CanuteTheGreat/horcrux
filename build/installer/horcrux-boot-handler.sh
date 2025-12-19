#!/bin/bash
#
# Horcrux Boot Mode Handler
# Runs at boot to check boot mode and start installer if needed
#
# This script is called by the init system to handle different boot modes:
#   - installer: Start interactive installer on TTY1
#   - autoinstall: Start automatic installation
#   - live: Normal live system boot
#

BOOT_MODE_FILE="/run/horcrux-boot-mode"
LOG_FILE="/var/log/horcrux-boot.log"

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1" | tee -a "$LOG_FILE"
}

# Get boot mode
get_boot_mode() {
    if [[ -f "$BOOT_MODE_FILE" ]]; then
        cat "$BOOT_MODE_FILE"
    else
        echo "live"
    fi
}

# Display welcome message on all TTYs
show_welcome() {
    local mode="$1"

    for tty in /dev/tty1 /dev/tty2 /dev/tty3; do
        if [[ -c "$tty" ]]; then
            cat > "$tty" << 'EOF'

  _    _
 | |  | |
 | |__| | ___  _ __ ___ _ __ _   ___  __
 |  __  |/ _ \| '__/ __| '__| | | \ \/ /
 | |  | | (_) | | | (__| |  | |_| |>  <
 |_|  |_|\___/|_|  \___|_|   \__,_/_/\_\

  Gentoo Virtualization Platform

EOF
        fi
    done
}

# Start installer on TTY1
start_installer() {
    local auto="$1"

    log "Starting installer (auto=$auto)..."

    # Clear TTY1
    clear > /dev/tty1

    # Set up TTY1 for installer
    if [[ "$auto" == "1" ]]; then
        # Find first suitable disk for automatic install
        local target_disk=""
        for disk in /dev/sda /dev/vda /dev/nvme0n1; do
            if [[ -b "$disk" ]]; then
                target_disk="$disk"
                break
            fi
        done

        if [[ -n "$target_disk" ]]; then
            log "Automatic install to $target_disk"
            /usr/bin/horcrux-installer --disk "$target_disk" --auto < /dev/tty1 > /dev/tty1 2>&1
        else
            log "No suitable disk found for automatic install"
            echo "ERROR: No suitable disk found for automatic installation" > /dev/tty1
            echo "Starting interactive installer instead..." > /dev/tty1
            sleep 3
            /usr/bin/horcrux-installer < /dev/tty1 > /dev/tty1 2>&1
        fi
    else
        # Interactive install
        /usr/bin/horcrux-installer < /dev/tty1 > /dev/tty1 2>&1
    fi
}

# Start getty for live mode
start_live_mode() {
    log "Starting live mode..."

    # Show information on TTY1
    cat > /dev/tty1 << 'EOF'

  Horcrux Live System
  ===================

  You are running Horcrux in live mode. Changes will not persist.

  To install Horcrux to disk, run:
    horcrux-installer

  Default credentials:
    Username: root
    Password: horcrux

  Web UI: http://localhost:8006

  Type 'horcrux --help' for CLI usage.

EOF

    # The normal getty will handle login
}

# Main
main() {
    local mode=$(get_boot_mode)

    log "Boot mode: $mode"
    show_welcome "$mode"

    case "$mode" in
        installer)
            start_installer 0
            ;;
        autoinstall)
            start_installer 1
            ;;
        live|*)
            start_live_mode
            ;;
    esac
}

main "$@"
