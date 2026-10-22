#!/bin/bash
# Save state on host shutdown / resume on boot for QEMU VMs, for real: the
# built horcrux-api runs in a throwaway privileged sibling container with real
# QEMU (KVM when the runner has it, else TCG) and CirrOS guests; see
# guest_state_e2e.py for what is checked. Nothing on the runner host is touched.
#
# Usage: horcrux-api/tests/guest_state_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
HERE=$(cd "$(dirname "$0")" && pwd)
NAME="horcrux-gs-e2e-$$"
CIRROS_URL=${CIRROS_URL:-https://download.cirros-cloud.net/0.6.2/cirros-0.6.2-x86_64-disk.img}

cleanup() { [ "${KEEP:-0}" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -f /tmp/$NAME-cirros.img; }
trap cleanup EXIT

DEV=(--device /dev/net/tun)
if [ -e /dev/kvm ]; then DEV+=(--device /dev/kvm); echo "KVM: yes"; else echo "KVM: no (TCG)"; fi
docker run -d --name "$NAME" --hostname gsnode --privileged "${DEV[@]}" \
  --memory 4g "$IMAGE" sleep 7200 >/dev/null
curl -sfL --retry 3 -o /tmp/$NAME-cirros.img "$CIRROS_URL"
docker exec "$NAME" mkdir -p /hx /var/lib/hx/images /var/lib/hx/vms
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$HERE/hx_node.py" "$NAME":/hx/hx_node.py
docker cp /tmp/$NAME-cirros.img "$NAME":/var/lib/hx/images/cirros.img

docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-utils iproute2 curl python3 libvirt0 >/dev/null'

# One env for the API and for `horcrux-api guests ...` (what the unit runs).
cat > /tmp/$NAME-env.sh <<'EOS'
export HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/var/lib/hx
export HORCRUX_DATABASE_URL=sqlite:///var/lib/hx/horcrux.db HORCRUX_SSL_DIR=/var/lib/hx/ssl
export HORCRUX_VM_STORAGE=/var/lib/hx/vms ADMIN_PASSWORD=admin RUST_LOG=info
export RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000
# The "host" boot id the test changes to simulate a reboot.
export HORCRUX_BOOT_ID_FILE=/var/lib/hx/bootid HORCRUX_GUEST_BOOT_DELAY_SECS=3
EOS
cat > /tmp/$NAME-start-api.sh <<'EOS'
#!/bin/bash
# start-api.sh: (re)start the API
. /hx/env.sh
if [ -f /var/lib/hx/api.pid ]; then kill $(cat /var/lib/hx/api.pid) 2>/dev/null || true; sleep 2; fi
nohup horcrux-api >> /var/lib/hx/api.log 2>&1 &
echo $! > /var/lib/hx/api.pid
rm -f /tmp/hx-token-*
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /var/lib/hx/api.log; exit 1
EOS
docker cp /tmp/$NAME-env.sh "$NAME":/hx/env.sh
docker cp /tmp/$NAME-start-api.sh "$NAME":/hx/start-api.sh
rm -f /tmp/$NAME-env.sh /tmp/$NAME-start-api.sh
docker exec "$NAME" bash -c 'echo boot-1 > /var/lib/hx/bootid; bash /hx/start-api.sh'

rc=0
N1="$NAME" python3 "$HERE/guest_state_e2e.py" || rc=$?
if [ $rc -ne 0 ]; then echo "---- api.log"; docker exec "$NAME" tail -150 /var/lib/hx/api.log || true; fi
exit $rc
