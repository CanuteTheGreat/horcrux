#!/bin/bash
# SAN real-effect test, host side: boots a throwaway Debian 13 VM (KVM) and runs san_guest_e2e.sh in it against a horcrux-api build.
#   bash san_vm_e2e.sh /path/horcrux-api [workdir]
# Needs qemu-system-x86_64, qemu-img, ssh, python3, curl and /dev/kvm (e.g. inside isoqa-qemu:1 with --device /dev/kvm).
# The VM image is cached in WORK/debian-13-generic-amd64.qcow2; the VM's disk is a throwaway overlay.
set -eu
BIN=$(readlink -f "$1"); WORK=${2:-/tmp/hxsan-vm}
HERE=$(cd "$(dirname "$0")" && pwd)
IMG_URL=${HXSAN_IMG_URL:-https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2}
SSH_PORT=${HXSAN_SSH_PORT:-22422}; HTTP_PORT=${HXSAN_HTTP_PORT:-18191}
mkdir -p "$WORK/seed"
cd "$WORK"
[ -s base.qcow2 ] || curl -fsSL --retry 5 -o base.qcow2.part "$IMG_URL" && { [ -s base.qcow2 ] || mv base.qcow2.part base.qcow2; }
rm -f disk.qcow2 key key.pub
qemu-img create -q -f qcow2 -F qcow2 -b base.qcow2 disk.qcow2 16G
ssh-keygen -q -t ed25519 -N '' -f key
cp "$BIN" seed/horcrux-api; objcopy --strip-debug seed/horcrux-api 2>/dev/null || true
cp "$HERE/hx_node.py" "$HERE/san_guest_e2e.sh" seed/
cat > seed/meta-data <<EOF
instance-id: hxsan-$(date +%s)
local-hostname: hxsan
EOF
cat > seed/user-data <<EOF
#cloud-config
ssh_authorized_keys:
  - $(cat key.pub)
disable_root: false
package_update: true
packages: [open-iscsi, multipath-tools, lvm2-lockd, sanlock, targetcli-fb, nftables, libvirt0, libpam0g, python3, qemu-utils, curl, util-linux]
runcmd:
  - [sh, -c, "for f in horcrux-api hx_node.py san_guest_e2e.sh; do curl -fsS http://10.0.2.2:${HTTP_PORT}/\$f -o /root/\$f; done; chmod 755 /root/horcrux-api; touch /root/hxsan-ready"]
EOF
python3 -m http.server "$HTTP_PORT" --bind 0.0.0.0 --directory seed > http.log 2>&1 &
HTTP=$!
trap 'kill $HTTP 2>/dev/null; [ -n "${QPID:-}" ] && kill $QPID 2>/dev/null; true' EXIT
qemu-system-x86_64 -machine q35,accel=kvm -cpu host -m ${HXSAN_MEM:-3072} -smp ${HXSAN_CPUS:-4} -display none \
  -drive file=disk.qcow2,if=virtio,format=qcow2 \
  -netdev user,id=n0,hostfwd=tcp:127.0.0.1:${SSH_PORT}-:22 -device virtio-net-pci,netdev=n0 \
  -smbios "type=1,serial=ds=nocloud;s=http://10.0.2.2:${HTTP_PORT}/" \
  -serial file:serial.log > qemu.log 2>&1 &
QPID=$!
S="ssh -i key -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o LogLevel=ERROR -p $SSH_PORT debian@127.0.0.1"
for i in $(seq 1 180); do
  $S 'test -e /root/hxsan-ready || sudo test -e /root/hxsan-ready' 2>/dev/null && break
  kill -0 $QPID 2>/dev/null || { echo "qemu died"; tail -30 qemu.log; exit 1; }
  sleep 5
done
$S 'sudo test -e /root/hxsan-ready' || { echo "VM not ready"; tail -60 serial.log; exit 1; }
$S 'sudo cloud-init status --wait >/dev/null 2>&1; uname -r; dpkg -l open-iscsi multipath-tools lvm2-lockd sanlock targetcli-fb | grep ^ii | awk "{print \$2, \$3}"'
set +e
$S 'sudo bash /root/san_guest_e2e.sh /root/horcrux-api /root/hx_node.py' 2>&1 | tee guest.log
rc=${PIPESTATUS[0]}
$S 'sudo tail -80 /var/lib/hxsan-e2e/api.log' > api-tail.log 2>&1
$S 'sudo poweroff' >/dev/null 2>&1
sleep 5
echo "EXIT_E2E=$rc"
exit $rc
