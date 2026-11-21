#!/bin/bash
# Horcrux Console desktop client on Windows against a real node (C3c,
# docs/design/horcrux-console.md): starts a standalone node with an Alpine
# console VM, boots the cached Windows 11 test image (ci/windows-test-image.py)
# as the client machine with user-mode networking, copies horcrux-console.exe
# in through the QEMU guest agent and runs it headless there, direct and
# through the node's WAN relay (hx_console_desktop_win_e2e.py).
#
# Usage: horcrux-api/tests/hx_console_desktop_win_e2e.sh <horcrux-api binary> <horcrux-console.exe>
# Env:   HXD_WIN_IMAGE  Windows test image (required; used read-only through an overlay)
#        HXD_WIN_MEM_MB / HXD_WIN_CPUS  client VM size (default 4096 / 4)
#        HXD_NODE_ADDR  node address the guest connects to (default: this
#                       machine's address on its default route)
#        HXD_RELAY_LISTEN  relay listen address for the relay run (an address,
#                       "all", or unset = automatic, see the .py)
#        HXC_CACHE      Alpine ISO cache (default /tmp/hxc-cache)
#        HXC_OUT        results, client logs, decoded snapshots (default /tmp/hxd-win-e2e)
set -euo pipefail
BIN=$(readlink -f "${1:?usage: $0 <horcrux-api binary> <horcrux-console.exe>}")
EXE=$(readlink -f "${2:?usage: $0 <horcrux-api binary> <horcrux-console.exe>}")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT=${HXC_OUT:-/tmp/hxd-win-e2e}
CACHE=${HXC_CACHE:-/tmp/hxc-cache}
DATA=$(mktemp -d /tmp/hxd-data.XXXXXX)
SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
mkdir -p "$OUT" "$CACHE" "$DATA/images" "$DATA/win/tpm"
[ -s "${HXD_WIN_IMAGE:-}" ] || { echo "FAIL HXD_WIN_IMAGE is not set or empty"; exit 1; }

need=()
for c in qemu-system-x86_64:qemu-system-x86 qemu-img:qemu-utils ffmpeg:ffmpeg swtpm:swtpm curl:curl ip:iproute2; do
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
    apt-cache policy qemu-system-x86 | grep -q 'Candidate: [0-9]' && break
    sleep 10
  done
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${need[@]}" >/dev/null
fi
echo "ok - QEMU $(qemu-system-x86_64 --version | head -1)"

# Alpine "virt" ISO: root console without a password (test fixture, as in
# hx_console_wan_e2e.sh)
AMIRROR=${HXC_ALPINE_MIRROR:-https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64}
AISO="$CACHE/alpine-virt.iso"
if [ ! -s "$AISO" ]; then
  meta=$(curl -fsSL --retry 3 "$AMIRROR/latest-releases.yaml")
  afile=$(printf '%s\n' "$meta" | awk '/flavor: alpine-virt/{f=1} f&&/file:/{print $2; exit}')
  asum=$(printf '%s\n' "$meta" | awk '/flavor: alpine-virt/{f=1} f&&/sha256:/{print $2; exit}')
  curl -fsSL --retry 3 -o "$AISO.part" "$AMIRROR/$afile"
  echo "$asum  $AISO.part" | sha256sum -c --quiet || { echo "FAIL Alpine ISO checksum"; exit 1; }
  mv "$AISO.part" "$AISO"
fi
cp "$AISO" "$DATA/images/alpine-virt.iso"

# The address the Windows guest connects to: QEMU's user-mode network
# forwards the guest's TCP/UDP to any address of this machine, so the
# node's own address on the default route is what a LAN client would use.
ADDR=${HXD_NODE_ADDR:-$(ip -4 route get 192.0.2.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") {print $(i + 1); exit}}')}
[ -n "$ADDR" ] || { echo "FAIL no node address (set HXD_NODE_ADDR)"; exit 1; }
echo "ok - node address as seen from the guest: $ADDR"

cleanup() {
  [ -f "$DATA/api.pid" ] && kill "$(cat "$DATA/api.pid")" 2>/dev/null || true
  [ -f "$DATA/win/qemu.pid" ] && kill "$(cat "$DATA/win/qemu.pid")" 2>/dev/null || true
  for p in $(pgrep -f "qemu-system-x86_64.*$DATA" || true); do kill "$p" 2>/dev/null || true; done
  for p in $(pgrep -f "swtpm.*$DATA" || true); do kill "$p" 2>/dev/null || true; done
  sleep 2
  rm -rf "$DATA"
}
trap cleanup EXIT

env HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_DATA_DIR="$DATA" \
  HORCRUX_DATABASE_URL="sqlite://$DATA/horcrux.db" HORCRUX_SSL_DIR="$DATA/ssl" \
  HORCRUX_VM_STORAGE="$DATA/vms" HORCRUX_CONSOLE_SETTINGS="$DATA/console.json" \
  ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 \
  RUST_LOG=info,horcrux_vm::console=debug,horcrux_turn=debug \
  nohup "$BIN" >> "$DATA/api.log" 2>&1 &
echo $! > "$DATA/api.pid"
for i in $(seq 1 90); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "FAIL API did not start"; tail -50 "$DATA/api.log"; exit 1; }

rc=0
HXD_EXE="$EXE" HXD_NODE_ADDR="$ADDR" HXD_WORK="$DATA/win" HXC_ALPINE_ISO="$DATA/images/alpine-virt.iso" \
  HXC_OUT="$OUT" python3 -u "$HERE/hx_console_desktop_win_e2e.py" || rc=$?
cp "$DATA/api.log" "$OUT/api.log" 2>/dev/null || true
if [ $rc -ne 0 ]; then
  echo "---- api.log (console lines, tail)"; grep -iE 'console|relay|turn' "$DATA/api.log" | tail -80
fi
exit $rc
