#!/bin/bash
# Gaming VM wizard end to end on real KVM (docs/design/gaming-vm.md phase 2):
# POST /api/vms/gaming composes a Windows 11 VM (Secure Boot OVMF with
# Microsoft keys, swtpm TPM 2.0, nested virtualisation for VBS, pinned vCPUs,
# virtio disk, guest agent, generated SMBIOS identity, answer-file disc),
# Windows installs unattended, the server notices the guest agent and deletes
# the answer disc, and gaming_wizard_e2e.py checks Secure Boot, TPM, VBS /
# Memory Integrity, SMBIOS and drivers from inside Windows.
#
# Runs where it is started (a CI job container or a capped container on a
# build host) with /dev/kvm and nested virtualisation. ISOs come from the
# ISO library: set ISO_DIR to a directory that already holds
# win11-eval.iso + virtio-win.iso to skip the downloads (about 6 GB).
#
# Usage: horcrux-api/tests/gaming_wizard_e2e.sh <horcrux-api binary>
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary>}")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DATA=${HX_DATA:-/tmp/hxg}
SUDO=; [ "$(id -u)" = 0 ] || SUDO=sudo

ls -l /dev/kvm
cat /sys/module/kvm_intel/parameters/nested /sys/module/kvm_amd/parameters/nested 2>/dev/null || true
need=
for p in qemu-system-x86_64:qemu-system-x86 qemu-img:qemu-utils swtpm:swtpm swtpm_setup:swtpm-tools \
         xorriso:xorriso curl:curl python3:python3; do
  command -v "${p%%:*}" >/dev/null || need="$need ${p#*:}"
done
[ -f /usr/share/OVMF/OVMF_VARS_4M.ms.fd ] || need="$need ovmf"
if [ -n "$need" ]; then
  $SUDO apt-get update -qq
  DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y -qq --no-install-recommends $need >/dev/null
fi

$SUDO rm -rf "$DATA"
mkdir -p "$DATA/vms"
IMG_DIR=${ISO_DIR:-$DATA/images}
mkdir -p "$IMG_DIR"
cleanup() {
  [ -f "$DATA/api.pid" ] && $SUDO kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  $SUDO pkill -f "[h]orcrux-vm-ci-gaming" 2>/dev/null || true
}
trap cleanup EXIT
$SUDO env HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_IMAGE_DIR="$IMG_DIR" ADMIN_PASSWORD=admin \
  RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
  bash -c "nohup '$BIN' >> '$DATA/api.log' 2>&1 & echo \$! > '$DATA/api.pid'"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 2; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }
$SUDO env HX_DATA="$DATA" MEMORY_MB="${MEMORY_MB:-4096}" python3 -u "$HERE/gaming_wizard_e2e.py" || {
  echo "---- api.log"; tail -120 "$DATA/api.log"; exit 1; }
