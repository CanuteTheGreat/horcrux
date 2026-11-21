#!/bin/bash
# Horcrux Console C2b end to end (guest tools in real Windows and Linux VMs,
# browser client): starts a standalone node and runs hx_console_guest_e2e.py.
#
# Usage: horcrux-api/tests/hx_console_guest_e2e.sh <horcrux-api binary> <ui dist dir>
# Env:   HXG_TOOLS_ISO   the guest tools CD (ci/build-guest-tools-iso.sh)
#        HXG_WIN_IMAGE   Windows test image (ci/windows-test-image.py), optional
#        HXG_LINUX_IMAGE Debian test image (ci/linux-test-image.py), optional
#        HXG_ONLY        windows | linux (default both)
#        HXC_OUT         screenshots/results (default /tmp/hxg-e2e)
#        HXC_PW_PY       python with playwright (installed into /tmp/hxc-pw if unset)
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> <ui dist dir>}")
UI=$(readlink -f "${2:?usage: $0 <horcrux-api binary> <ui dist dir>}")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT=${HXC_OUT:-/tmp/hxg-e2e}
DATA=$(mktemp -d /tmp/hxg-data.XXXXXX)
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
mkdir -p "$OUT" "$DATA/images" "$DATA/guest-tools"
[ -s "${HXG_TOOLS_ISO:-}" ] || { echo "FAIL HXG_TOOLS_ISO is not set or empty"; exit 1; }

need=()
for c in qemu-system-x86_64:qemu-system-x86 qemu-img:qemu-utils ffmpeg:ffmpeg swtpm:swtpm swtpm_setup:swtpm-tools curl:curl; do
  command -v "${c%%:*}" >/dev/null || need+=("${c#*:}")
done
[ -e /usr/share/OVMF/OVMF_CODE_4M.secboot.fd ] || need+=(ovmf)
ldd "$BIN" 2>/dev/null | grep -q 'libvirt.so.* not found' && need+=(libvirt0)
if ! command -v qemu-system-x86_64 >/dev/null || ! qemu-system-x86_64 -display help | grep -qx dbus; then
  need+=(qemu-system-modules-opengl)
fi
if [ ${#need[@]} -gt 0 ]; then
  for i in 1 2 3; do
    $SUDO apt-get update -qq >/dev/null 2>&1 || true
    apt-cache policy ffmpeg | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${need[@]}" python3-venv >/dev/null
fi
echo "ok - QEMU $(qemu-system-x86_64 --version | head -1)"

PY=${HXC_PW_PY:-}
if [ -z "$PY" ]; then
  if [ ! -x /tmp/hxc-pw/bin/python ]; then
    python3 -m venv /tmp/hxc-pw
    /tmp/hxc-pw/bin/pip install -q playwright
    $SUDO /tmp/hxc-pw/bin/playwright install-deps chromium >/dev/null
  fi
  /tmp/hxc-pw/bin/playwright install chromium >/dev/null
  PY=/tmp/hxc-pw/bin/python
fi

# images go into the node's image library (import sources must live there)
link() { ln "$1" "$2" 2>/dev/null || cp --sparse=always "$1" "$2"; }
cp "$HXG_TOOLS_ISO" "$DATA/guest-tools/horcrux-guest-tools.iso"
WIN_IMG=""; LIN_IMG=""
[ -s "${HXG_WIN_IMAGE:-}" ] && { link "$HXG_WIN_IMAGE" "$DATA/images/win-test.qcow2"; WIN_IMG="$DATA/images/win-test.qcow2"; }
[ -s "${HXG_LINUX_IMAGE:-}" ] && { link "$HXG_LINUX_IMAGE" "$DATA/images/debian-test.qcow2"; LIN_IMG="$DATA/images/debian-test.qcow2"; }

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  for p in $(pgrep -f "qemu-system-x86_64.*$DATA" || true); do kill "$p" 2>/dev/null || true; done
  for p in $(pgrep -f "swtpm.*$DATA" || true); do kill "$p" 2>/dev/null || true; done
  sleep 2
  rm -rf "$DATA"
}
trap cleanup EXIT

env HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_CONSOLE_SETTINGS="$DATA/console.json" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  RUST_LOG=info,horcrux_vm::console=debug HORCRUX_UI_DIR="$UI" \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }

rc=0
HXG_IMAGES="$DATA/images" HXG_TOOLS_ISO="$DATA/guest-tools/horcrux-guest-tools.iso" \
  HXG_WIN_IMAGE="$WIN_IMG" HXG_LINUX_IMAGE="$LIN_IMG" HXC_OUT="$OUT" \
  "$PY" -u "$HERE/hx_console_guest_e2e.py" || rc=$?
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
if [ $rc -ne 0 ]; then
  echo "---- api.log (console lines, tail)"; grep -iE 'console|guest tools|hxg' "$DATA/api.log" | tail -80
fi
exit $rc
