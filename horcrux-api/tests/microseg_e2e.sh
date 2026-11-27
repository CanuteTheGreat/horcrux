#!/bin/bash
# Guest microsegmentation against the real kernel: run the built horcrux-api
# in a throwaway privileged sibling container (its own network namespace;
# nothing on the runner host is touched), build a bridge with guest and peer
# network namespaces, and check that a denied flow is dropped, an allowed one
# passes and the per-rule counters move (microseg_e2e.py).
#
# Usage: horcrux-api/tests/microseg_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-26.04}
NAME="horcrux-mseg-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --privileged --memory 1g "$IMAGE" sleep 900 >/dev/null
hx_sibling_apt_prep "$NAME"
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker exec "$NAME" bash -c '(command -v nft && command -v ip && command -v curl && command -v python3 && command -v qemu-img && command -v conntrack && ldconfig -p | grep -q libvirt.so.0) >/dev/null || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nftables iproute2 conntrack curl python3 qemu-utils libvirt0 >/dev/null)'
docker cp "$(dirname "$0")/microseg_e2e.py" "$NAME":/tmp/microseg_e2e.py

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/ms && cd /tmp/ms
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR=/tmp/ms \
HORCRUX_DATABASE_URL=sqlite:///tmp/ms/horcrux.db HORCRUX_SSL_DIR=/tmp/ms/ssl ADMIN_PASSWORD=admin RUST_LOG=warn \
  nohup horcrux-api > /tmp/ms/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
curl -skf https://127.0.0.1:8006/api/health >/dev/null
'
docker exec "$NAME" python3 /tmp/microseg_e2e.py || { docker exec "$NAME" tail -60 /tmp/ms/api.log; exit 1; }
