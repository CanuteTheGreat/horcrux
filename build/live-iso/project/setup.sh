#!/bin/sh
set -e
chmod +x /etc/init.d/horcrux-api
for s in horcrux-api libvirtd docker; do rc-update add $s default >/dev/null 2>&1 || echo "warn: rc-update add $s"; done
mkdir -p /var/lib/horcrux /var/log/horcrux /etc/horcrux/ssl
# ZFS (root-on-ZFS / mirror installs): module + tools + dracut hook must be in the image
find /lib/modules -name 'zfs.ko*' | grep -q . || { echo "zfs.ko missing for the installed kernel"; exit 1; }
command -v zpool >/dev/null && command -v zgenhostid >/dev/null || { echo "zfs userland missing"; exit 1; }
