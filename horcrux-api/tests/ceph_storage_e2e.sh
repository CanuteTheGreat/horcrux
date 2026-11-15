#!/bin/bash
# Ceph RBD storage pool end to end against a real (single-node) Ceph cluster in a
# container: mon + mgr + one bluestore OSD, the API runs next to it with the ceph/rbd
# CLIs. Adds a pool by RADOS pool name, checks the refused cases, that the pool reports
# the cluster's real capacity, creates / lists / grows / deletes RBD images through the
# API and checks each with `rbd info`.
# Usage: horcrux-api/tests/ceph_storage_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-ubuntu:24.04}
TAG="${HX_RUN_TAG:-$$}"
CT="horcrux-ceph-e2e-$TAG"
PORT=$((20000 + RANDOM % 20000))
HERE=$(dirname "$0")

cleanup() {
  docker rm -fv "$CT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --name "$CT" --memory 4g --memory-swap 4g --cpus 3 "$IMAGE" sleep 1800 >/dev/null
docker cp "$BIN" "$CT":/usr/local/bin/horcrux-api
docker cp "$HERE/ceph_cluster_boot.sh" "$CT":/tmp/ceph_cluster_boot.sh
docker cp "$HERE/ceph_storage_e2e.py" "$CT":/tmp/ceph_storage_e2e.py

docker exec "$CT" bash -c 'DEBIAN_FRONTEND=noninteractive apt-get update -qq && apt-get install -y -qq \
  qemu-utils iproute2 curl python3 libvirt0 udev >/dev/null'
docker exec "$CT" bash /tmp/ceph_cluster_boot.sh || { echo "FAIL the Ceph test cluster did not come up"; exit 1; }
echo "ok - single-node Ceph cluster is up (hxpool)"

docker exec -e PORT="$PORT" "$CT" bash -c '
set -euo pipefail
mkdir -p /tmp/dev && cd /tmp/dev
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=$PORT HORCRUX_DATA_DIR=/tmp/dev \
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \
HORCRUX_VM_STORAGE=/tmp/dev/vms ADMIN_PASSWORD=admin RUST_LOG=info \
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:$PORT/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:$PORT/api/health >/dev/null || { echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1; }
python3 /tmp/ceph_storage_e2e.py || { echo "---- api.log"; tail -60 /tmp/dev/api.log; ceph -s; exit 1; }'
