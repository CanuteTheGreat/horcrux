#!/bin/bash
# Prepare guest image + cloud-init seed inside the container (/w).
set -e
cd /w
[ -f guestkey ] || ssh-keygen -q -t ed25519 -N "" -f guestkey -C hxvgpu-spike
[ -f arch.qcow2 ] || curl -fsSL -o arch.qcow2 https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2
[ -f guest.qcow2 ] || { cp arch.qcow2 guest.qcow2; qemu-img resize guest.qcow2 20G; }
mkdir -p seed
cat > seed/meta-data <<EOF
instance-id: hxvgpu-1
local-hostname: hxvgpu
EOF
cat > seed/user-data <<EOF
#cloud-config
disable_root: false
ssh_pwauth: false
users:
  - name: root
    ssh_authorized_keys:
      - $(cat /w/guestkey.pub)
EOF
xorriso -as mkisofs -quiet -o seed.iso -V cidata -J -r seed
ls -l guest.qcow2 seed.iso
