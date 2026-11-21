#!/bin/bash
# Remote Play ready (docs/design/gaming-vm.md section 16), for real: runs the
# built horcrux-api with real QEMU (TCG) and real nftables in a throwaway
# privileged sibling container (its own network namespace; nothing on the
# runner host is touched), with two client network namespaces around it:
#
#   lan    192.168.77.0/24  an allowed network (a Steam Link / Moonlight client)
#   other  10.9.9.0/24      a network that is not allowed
#   vm     10.50.0.10       stands in for the guest's network stack behind a
#                           NAT bridge (same MAC as the VM's card)
#
# Checks: a Wake-on-LAN magic packet from "other" does not start the VM, one
# from "lan" does; a Steam discovery request from "lan" reaches the VM through
# the relay and the VM's answer comes back to the client; the forwarded TCP
# Remote Play port reaches the VM from "lan" and not from "other"; the
# checklist and rule list report it; turning the option off removes the
# nftables table and the listeners.
# Usage: horcrux-api/tests/remote_play_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-26.04}
NAME="horcrux-remoteplay-e2e-${HX_RUN_TAG:-$$}"

cleanup() { [ -n "${HX_KEEP:-}" ] || docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --memory 1g --memory-swap 1g "$IMAGE" sleep 1800 >/dev/null
hx_sibling_apt_prep "$NAME"
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/remote_play_e2e.py" "$NAME":/tmp/remote_play_e2e.py
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-utils iproute2 iputils-ping nftables curl python3 libvirt0 >/dev/null'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/rp && cd /tmp/rp
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/rp \
HORCRUX_DATABASE_URL=sqlite:///tmp/rp/horcrux.db HORCRUX_SSL_DIR=/tmp/rp/ssl \
HORCRUX_VM_STORAGE=/tmp/rp/vms ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/rp/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null || { echo "API did not start"; tail -50 /tmp/rp/api.log; exit 1; }
python3 /tmp/remote_play_e2e.py || { echo "---- api.log"; tail -80 /tmp/rp/api.log; exit 1; }
'
