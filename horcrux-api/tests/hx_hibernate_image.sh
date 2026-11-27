#!/bin/bash
# Build the Debian guest the guest-hibernate e2e uses: the stock Debian cloud image plus qemu-guest-agent, the Horcrux guest
# agent binary, cloud-init off (the e2e talks to the guest over the guest-agent channel, not ssh). Needs qemu + /dev/kvm
# (or TCG), ssh, curl, python3: run it in isoqa-qemu:1 on the CI host.
#
#   hx_hibernate_image.sh HORCRUX_GUEST_AGENT_BINARY OUT.qcow2 [WORKDIR]
#
# The image is NOT prepared for hibernation: the e2e does that with `horcrux-guest-agent --hibernate-enable`, which is
# part of what it proves.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
agent=$1 out=$2
export HX_VM_DIR=${3:-/tmp/hx-hib-img-$$}
. "$here/hx_vm.sh"
trap hx_vm_down EXIT
hx_vm_prepare
hx_vm_up hib "${HX_VM_PORT:-2288}" 2048 2
hx_vm_scp "$agent" hib:/usr/bin/horcrux-guest-agent
hx_vm_ssh hib 'set -e
chmod 755 /usr/bin/horcrux-guest-agent
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq qemu-guest-agent initramfs-tools e2fsprogs util-linux >/dev/null
systemctl enable qemu-guest-agent >/dev/null 2>&1 || true
# no first-boot network waits: nothing in the e2e needs cloud-init after this
touch /etc/cloud/cloud-init.disabled
cloud-init clean --logs >/dev/null 2>&1 || true
# a serial console for diagnosing a failed resume
systemctl enable serial-getty@ttyS0.service >/dev/null 2>&1 || true
sync'
hx_vm_ssh hib 'systemctl poweroff' || true
pid=${HX_VM_PIDS[-1]}
for i in $(seq 1 60); do kill -0 "$pid" 2>/dev/null || break; sleep 2; done
kill "$pid" 2>/dev/null || true
qemu-img convert -O qcow2 -c "$HX_VM_DIR/hib.qcow2" "$out.part" && mv "$out.part" "$out"
rm -f "$HX_VM_DIR/hib.qcow2"
ls -l "$out"
