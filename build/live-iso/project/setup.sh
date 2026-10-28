#!/bin/sh
set -e
chmod +x /etc/init.d/horcrux-api /usr/libexec/live-install-hook /usr/bin/horcrux-installer
for s in horcrux-api libvirtd docker; do rc-update add $s default >/dev/null 2>&1 || echo "warn: rc-update add $s"; done
mkdir -p /var/lib/horcrux /var/log/horcrux /etc/horcrux/ssl
