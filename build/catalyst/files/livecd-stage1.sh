#!/bin/bash
# Horcrux LiveCD Stage1 Customization Script
# This runs inside the chroot after packages are installed

set -e

echo "Horcrux LiveCD Stage1 customization starting..."

# Set timezone
ln -sf /usr/share/zoneinfo/UTC /etc/localtime

# Set locale
echo 'en_US.UTF-8 UTF-8' > /etc/locale.gen
locale-gen
eselect locale set en_US.utf8

# Configure hostname
echo 'horcrux' > /etc/hostname

# Configure hosts file
cat > /etc/hosts << 'EOF'
127.0.0.1   localhost horcrux
::1         localhost horcrux
EOF

# Set root password (horcrux)
echo 'root:horcrux' | chpasswd

# Create horcrux user
useradd -m -G wheel -s /bin/bash horcrux || true
echo 'horcrux:horcrux' | chpasswd

# Configure sudo
mkdir -p /etc/sudoers.d
echo '%wheel ALL=(ALL) ALL' > /etc/sudoers.d/wheel
chmod 440 /etc/sudoers.d/wheel

# Enable essential services
rc-update add sshd default
rc-update add dhcpcd default
rc-update add sysklogd default
rc-update add cronie default

# Create motd
cat > /etc/motd << 'EOF'

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

# Create basic network configuration
cat > /etc/conf.d/net << 'EOF'
# DHCP on all interfaces by default
config_eth0="dhcp"
config_ens3="dhcp"
EOF

# Cleanup
rm -rf /var/cache/distfiles/*
rm -rf /var/tmp/portage/*

echo "Horcrux LiveCD Stage1 customization complete."
