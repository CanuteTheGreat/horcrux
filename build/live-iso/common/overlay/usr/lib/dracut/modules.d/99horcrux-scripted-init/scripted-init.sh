#!/bin/sh
# script-based initramfs (see module-setup.sh): no systemctl, so no hook mistakes this initrd for a systemd one
[ -d /run/systemd/system ] || rm -f /usr/bin/systemctl /bin/systemctl
