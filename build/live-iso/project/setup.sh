#!/bin/sh
set -e
chmod +x /etc/init.d/horcrux-api
# The product services are NOT in the live runlevel: the installer enables them on the installed system (profile services),
# "Try it live" starts them through live-product with a random admin password, and the rescue / installer / forensic
# modes run without the product (no default admin account, nothing touching storage).
mkdir -p /var/lib/horcrux /var/log/horcrux /etc/horcrux/ssl
# ZFS (root-on-ZFS / mirror installs): module + tools + dracut hook must be in the image
find /lib/modules -name 'zfs.ko*' | grep -q . || { echo "zfs.ko missing for the installed kernel"; exit 1; }
command -v zpool >/dev/null && command -v zgenhostid >/dev/null || { echo "zfs userland missing"; exit 1; }
