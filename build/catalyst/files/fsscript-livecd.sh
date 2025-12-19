#!/bin/bash
#
# Horcrux LiveCD Customization Script
# Runs inside the chroot during livecd-stage2
#

set -e

echo "Horcrux LiveCD customization starting..."

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
useradd -m -G wheel,audio,video,plugdev,usb -s /bin/bash horcrux 2>/dev/null || true
echo 'horcrux:horcrux' | chpasswd

# Configure sudo
mkdir -p /etc/sudoers.d
echo '%wheel ALL=(ALL) ALL' > /etc/sudoers.d/wheel
chmod 440 /etc/sudoers.d/wheel

# Create basic network configuration for DHCP
cat > /etc/conf.d/net << 'EOF'
# DHCP on all interfaces by default
config_eth0="dhcp"
config_ens3="dhcp"
EOF

# Clean up
rm -rf /var/cache/distfiles/*
rm -rf /var/tmp/portage/*

echo "Horcrux LiveCD customization complete."
