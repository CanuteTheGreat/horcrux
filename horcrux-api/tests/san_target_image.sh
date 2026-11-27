#!/bin/bash
# Build the SAN array VM the multi-node SAN vmtest uses (tools/vmtest/scenarios/san_multipath.py): the stock Debian 13 cloud
# image plus targetcli-fb (LIO iSCSI target) and nvme-cli, a root password for the serial console, cloud-init off. The NVMe/TCP
# target (nvmet) is the stock kernel's; the scenario configures LIO and nvmet at run time over the serial console.
# Needs qemu + /dev/kvm (or TCG), ssh, curl, python3: run it in isoqa-qemu:1.
#
#   san_target_image.sh OUT.qcow2 ROOT_PASSWORD [WORKDIR]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$1 pw=$2
export HX_VM_DIR=${3:-/tmp/hx-san-tgt-$$}
. "$here/hx_vm.sh"
trap hx_vm_down EXIT
hx_vm_prepare
hx_vm_up sant "${HX_VM_PORT:-2289}" 2048 2
printf '%s\n' "root:$pw" > "$HX_VM_DIR/chpw"
hx_vm_scp "$HX_VM_DIR/chpw" sant:/root/chpw
hx_vm_ssh sant 'set -e
export DEBIAN_FRONTEND=noninteractive
for i in 1 2 3 4 5; do apt-get update -qq && apt-cache policy targetcli-fb | grep -q "Candidate: [0-9]" && break; sleep 10; done
apt-get install -y -qq targetcli-fb nvme-cli util-linux kmod iproute2 >/dev/null
chpasswd < /root/chpw; rm -f /root/chpw
# a serial root login for the scenario (it drives the array over ttyS0 only)
systemctl enable serial-getty@ttyS0.service >/dev/null 2>&1 || true
# LIO and nvmet modules load at boot (configfs comes with them)
printf "%s\n" target_core_mod iscsi_target_mod target_core_file nvmet nvmet_tcp > /etc/modules-load.d/hx-san-target.conf
# nothing in the scenario needs cloud-init after this
touch /etc/cloud/cloud-init.disabled
cloud-init clean --logs >/dev/null 2>&1 || true
sync'
hx_vm_ssh sant 'systemctl poweroff' || true
pid=${HX_VM_PIDS[-1]}
for i in $(seq 1 60); do kill -0 "$pid" 2>/dev/null || break; sleep 2; done
kill "$pid" 2>/dev/null || true
qemu-img convert -O qcow2 -c "$HX_VM_DIR/sant.qcow2" "$out.part" && mv "$out.part" "$out"
rm -f "$HX_VM_DIR/sant.qcow2"
ls -l "$out"
