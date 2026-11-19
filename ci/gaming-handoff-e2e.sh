#!/bin/bash
# Real-kernel end-to-end check of the gaming-VM host plumbing
# (docs/design/gaming-vm.md section 8).
#
# Boots a throw-away Debian 13 VM with an emulated IOMMU (intel-iommu,
# caching-mode for VFIO) and a spare PCI display (bochs-display on its own
# root port, driven by bochs-drm), then runs the ignored `handoff_e2e` test of
# horcrux-vm in it as root: managed hand-off (refused while a program holds
# the card, then bochs-drm -> vfio-pci), a nested QEMU with the device +
# Hyper-V flags + huge-page memory, CPU pinning from QMP thread ids, release
# waiting for QEMU, and the device back on bochs-drm afterwards.
#
# Needs /dev/kvm with nested virtualisation, qemu-system-x86_64, qemu-img,
# ssh, curl and python3 (the isoqa-qemu CI image has them):
#
#   cargo test -p horcrux-vm --features qemu --test handoff_e2e --no-run
#   ci/gaming-handoff-e2e.sh target/debug/deps/handoff_e2e-<hash>
set -euo pipefail
BIN=$(readlink -f "$1")
W=${WORK:-$(mktemp -d)}
IMG_URL=${IMG_URL:-https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2}
PORT=${SSH_PORT:-22220}
HTTP=${HTTP_PORT:-18000}
cd "$W"
IMG=$(basename "$IMG_URL")
[ -s "$IMG" ] || curl -fsSL -o "$IMG" "$IMG_URL"
rm -f l1.qcow2 key key.pub
qemu-img create -q -f qcow2 -b "$IMG" -F qcow2 l1.qcow2 12G
ssh-keygen -q -t ed25519 -N '' -f key
mkdir -p seed
echo "instance-id: hx-handoff-$$" > seed/meta-data
cat > seed/user-data <<EOF
#cloud-config
disable_root: false
ssh_pwauth: false
users:
  - name: root
    ssh_authorized_keys: ["$(cat key.pub)"]
package_update: true
packages: [qemu-system-x86, pciutils]
runcmd:
  - sed -i 's/^GRUB_CMDLINE_LINUX_DEFAULT="/&intel_iommu=on iommu=pt /' /etc/default/grub
  - update-grub
  - touch /root/provisioned
EOF
(cd seed && exec python3 -m http.server "$HTTP" --bind 127.0.0.1 >/dev/null 2>&1) &
HTTPD=$!
trap 'kill $HTTPD 2>/dev/null || true; [ -f l1.pid ] && kill $(cat l1.pid) 2>/dev/null || true' EXIT

boot() {
  qemu-system-x86_64 -machine q35,accel=kvm,kernel-irqchip=split -cpu host -smp 4 -m 4096 \
    -device intel-iommu,intremap=on,caching-mode=on \
    -drive file=l1.qcow2,if=virtio \
    -smbios "type=1,serial=ds=nocloud;s=http://10.0.2.2:$HTTP/" \
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$PORT-:22 \
    -vga std \
    -device pcie-root-port,id=rp1,chassis=1,slot=1 -device bochs-display,bus=rp1 \
    -display none -serial file:l1-serial.log -daemonize -pidfile l1.pid
}
SSH="ssh -i key -p $PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o LogLevel=ERROR root@127.0.0.1"
wait_for() { # wait_for SECONDS COMMAND...
  local t=$1; shift
  for _ in $(seq 1 $((t / 5))); do "$@" >/dev/null 2>&1 && return 0; sleep 5; done
  return 1
}

boot
wait_for 900 $SSH test -f /root/provisioned || { echo "FAIL provisioning"; tail -30 l1-serial.log; exit 1; }
echo "ok - L1 provisioned (qemu, IOMMU kernel option)"
$SSH 'systemctl reboot' || true
sleep 10
wait_for 300 $SSH grep -q intel_iommu=on /proc/cmdline || { echo "FAIL no IOMMU after restart"; exit 1; }
$SSH 'modprobe vfio-pci; ls /sys/kernel/iommu_groups | wc -l; lspci -nnk -d 1234:1111'
ADDR=$($SSH "lspci -D -n -d 1234:1111 | awk '\$2 ~ /^0380|^0300/ {print \$1}' | tail -1")
[ -n "$ADDR" ] || ADDR=$($SSH "lspci -D -n -d 1234:1111 | tail -1 | cut -d' ' -f1")
echo "ok - spare display at $ADDR, driver $($SSH readlink /sys/bus/pci/devices/$ADDR/driver | xargs basename)"
scp -q -i key -P "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$BIN" root@127.0.0.1:/root/handoff_e2e
$SSH "HX_HANDOFF_E2E_DEVICE=$ADDR /root/handoff_e2e --ignored --nocapture --test-threads 1" 2>&1 | tee e2e.log
grep -q '^test result: ok. 1 passed' e2e.log && echo "PASS gaming-handoff-e2e" || { echo "FAIL gaming-handoff-e2e"; exit 1; }
