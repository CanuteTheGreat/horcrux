#!/bin/bash
# iSCSI storage pool end to end against a real iSCSI target (tgt, userspace, in its
# own container on a private docker network) and the real kernel initiator
# (open-iscsi + iscsi_tcp) in the API's container: add the pool by
# iscsi://portal/iqn, see its LUNs as volumes with their real size, write through
# the volume's device path and find the bytes in the target's backing file, remove
# the pool and check the session and the devices are gone.
# The initiator container shares the host network namespace (iSCSI sessions belong
# to the kernel's initial namespace; the daemon's abstract socket lives there too, so
# the host's socket-activated iscsid does the work), the host /dev (the LUN block
# devices appear there) and /var/lib/iscsi (the node database iscsiadm and the host
# iscsid must share). The test logs out and deletes its node record before it removes
# anything. Skips (exit 0) when the host cannot load iscsi_tcp or has no iscsid.
# Usage: horcrux-api/tests/iscsi_storage_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-ubuntu:24.04}
TAG="${HX_RUN_TAG:-$$}"
NET="horcrux-iscsi-e2e-$TAG"; SRV="horcrux-iscsi-srv-$TAG"; CLI="horcrux-iscsi-cli-$TAG"
IQN="iqn.2026-11.test.horcrux:e2e"
PORT=$((20000 + RANDOM % 20000))

cleanup() {
  docker exec "$CLI" sh -c 'pkill horcrux-api; sleep 1; iscsiadm -m node -T '"$IQN"' -u >/dev/null 2>&1; iscsiadm -m node -T '"$IQN"' -o delete >/dev/null 2>&1' >/dev/null 2>&1 || true
  docker rm -fv "$CLI" >/dev/null 2>&1 || true
  docker rm -fv "$SRV" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if ! docker run --rm --privileged -v /lib/modules:/lib/modules:ro "$IMAGE" sh -c 'apt-get update -qq && apt-get install -y -qq kmod >/dev/null && modprobe iscsi_tcp' >/dev/null 2>&1; then
  echo "SKIP iscsi_tcp cannot be loaded on this host"; exit 0
fi

docker network create "$NET" >/dev/null
docker run -d --name "$SRV" --network "$NET" --privileged --memory 1g "$IMAGE" sleep 1800 >/dev/null
docker run -d --name "$CLI" --network host --privileged -v /dev:/dev -v /var/lib/iscsi:/var/lib/iscsi --memory 1g "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$CLI":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/iscsi_storage_e2e.py" "$CLI":/tmp/iscsi_storage_e2e.py

docker exec "$SRV" bash -c "
set -euo pipefail
apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq tgt >/dev/null
mkdir -p /srv/luns
truncate -s 1G /srv/luns/a.img; truncate -s 2G /srv/luns/b.img
tgtd
for i in \$(seq 1 20); do tgtadm --lld iscsi --op show --mode system >/dev/null 2>&1 && break; sleep 1; done
tgtadm --lld iscsi --op new --mode target --tid 1 -T $IQN
tgtadm --lld iscsi --op new --mode logicalunit --tid 1 --lun 1 -b /srv/luns/a.img
tgtadm --lld iscsi --op new --mode logicalunit --tid 1 --lun 2 -b /srv/luns/b.img
tgtadm --lld iscsi --op bind --mode target --tid 1 -I ALL"
SRV_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$SRV")

docker exec "$CLI" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  open-iscsi qemu-utils iproute2 curl python3 libvirt0 udev >/dev/null'
if docker exec "$CLI" iscsiadm -m session 2>&1 | grep -qi "can not connect\|not connect to iSCSI daemon"; then
  echo "SKIP the host has no iscsid to talk to"; exit 0
fi
docker exec -e SRV_IP="$SRV_IP" -e IQN="$IQN" -e PORT="$PORT" "$CLI" bash -c '
set -euo pipefail
mkdir -p /tmp/dev && cd /tmp/dev
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=$PORT HORCRUX_DATA_DIR=/tmp/dev \
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \
HORCRUX_VM_STORAGE=/tmp/dev/vms ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:$PORT/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:$PORT/api/health >/dev/null || { echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1; }
python3 /tmp/iscsi_storage_e2e.py create || { echo "---- api.log"; tail -60 /tmp/dev/api.log; iscsiadm -m session -P1 | head -30; exit 1; }'

# the bytes written through the volume's device path are in the target's backing file
docker exec "$SRV" bash -c 'head -c 32 /srv/luns/a.img | grep -aq HORCRUX-ISCSI-E2E' || { echo "FAIL pattern not in the target backing file"; exit 1; }
echo "ok - bytes written through lun1 are in the target's backing file"
docker exec -e SRV_IP="$SRV_IP" -e IQN="$IQN" -e PORT="$PORT" "$CLI" python3 /tmp/iscsi_storage_e2e.py delete
docker exec "$CLI" bash -c '! iscsiadm -m session 2>/dev/null | grep -q "'"$IQN"'"' || { echo "FAIL removing the pool left the iSCSI session"; exit 1; }
echo "ok - removing the pool logged out of the target"
