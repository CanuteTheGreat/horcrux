#!/bin/bash
# Real firewall apply: run the built horcrux-api in a throwaway sibling
# container that has CAP_NET_ADMIN (its own network namespace, so nothing on
# the runner host is touched), add a rule through the API, POST
# /api/firewall/datacenter/apply, and check the rule is really in the kernel's
# nftables ruleset. CI's Test job container is unprivileged, so the
# integration test can only check the endpoint's error path there.
#
# Usage: horcrux-api/tests/firewall_nft_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
NAME="horcrux-nft-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --cap-add NET_ADMIN --memory 1g "$IMAGE" sleep 900 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker exec "$NAME" bash -c '(command -v nft && ldconfig -p | grep -q libvirt.so.0) >/dev/null || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nftables curl libvirt0 >/dev/null)'
docker exec "$NAME" bash -c 'command -v python3 >/dev/null || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3 >/dev/null)'
docker cp "$(dirname "$0")/firewall_objects_e2e.py" "$NAME":/tmp/firewall_objects_e2e.py

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/fw && cd /tmp/fw
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/fw \
HORCRUX_DATABASE_URL=sqlite:///tmp/fw/horcrux.db HORCRUX_SSL_DIR=/tmp/fw/ssl ADMIN_PASSWORD=admin RUST_LOG=warn \
  nohup horcrux-api > /tmp/fw/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && break; sleep 1; done
TOKEN=$(curl -skf -H "Content-Type: application/json" -d "{\"username\":\"admin\",\"password\":\"admin\"}" \
  https://127.0.0.1:8006/api/auth/login | sed -E "s/.*\"ticket\":\"([^\"]+)\".*/\1/")
curl -skf -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d "{\"id\":\"nft-e2e\",\"enabled\":true,\"action\":\"ACCEPT\",\"direction\":\"in\",\"protocol\":\"tcp\",\"source\":null,\"dest\":null,\"sport\":null,\"dport\":\"8443\",\"comment\":\"nft-e2e\",\"log\":false,\"position\":0}" \
  https://127.0.0.1:8006/api/firewall/rules >/dev/null
code=$(curl -sk -o /tmp/fw/apply.out -w "%{http_code}" -X POST -H "Authorization: Bearer $TOKEN" \
  https://127.0.0.1:8006/api/firewall/datacenter/apply)
if [ "$code" != 200 ]; then echo "apply returned $code: $(cat /tmp/fw/apply.out)"; tail -50 /tmp/fw/api.log; exit 1; fi
nft list ruleset > /tmp/fw/ruleset
if ! grep -q "dport 8443" /tmp/fw/ruleset; then
  echo "rule not in nftables ruleset:"; cat /tmp/fw/ruleset; tail -50 /tmp/fw/api.log; exit 1
fi
grep -q "table inet horcrux\|table ip horcrux\|table horcrux" /tmp/fw/ruleset || { echo "no horcrux table"; cat /tmp/fw/ruleset; exit 1; }
echo "firewall apply: rule present in kernel nftables ruleset"
'

# IP sets, aliases, macros and default policies, checked against the kernel ruleset
docker exec "$NAME" python3 /tmp/firewall_objects_e2e.py || { docker exec "$NAME" tail -50 /tmp/fw/api.log; exit 1; }
