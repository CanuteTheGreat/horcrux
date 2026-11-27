#!/bin/bash
# Security events, for real: failed logins from a second container are
# counted, the brute-force detector fires, blocks the client in the kernel's
# nftables ruleset (the client really loses its route to the API, the
# server's own loopback keeps working), unblock restores it, a block is put
# back after an API restart, and unsafe addresses are refused. The built
# horcrux-api runs as root in a NET_ADMIN sibling (its own network namespace,
# nothing on the runner host is touched); the client is a second container on
# a throwaway docker network. CI's Test container is unprivileged, so the
# integration test (tests/it/security_activity_tests.rs) covers the rest.
# Usage: horcrux-api/tests/security_activity_e2e.sh <horcrux-api binary> [image]
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN="${1:?usage: $0 <horcrux-api binary> [image]}"
IMG="${2:-catthehacker/ubuntu:act-26.04}"
N="hx-secev-e2e-${HX_RUN_TAG:-$$}"
trap 'cleanup' EXIT
fail=0
NFTTXT=$(mktemp)
ok() { echo "ok - $*"; }
bad() { echo "FAIL - $*"; fail=1; }
cleanup() { rm -f "$NFTTXT"; docker rm -f $N-srv $N-cli >/dev/null 2>&1; docker network rm $N >/dev/null 2>&1; }
cleanup
docker network create $N >/dev/null || exit 2
docker run -d --name $N-srv --network $N --cap-add NET_ADMIN $IMG sleep 3600 >/dev/null
docker cp "$BIN" $N-srv:/usr/local/bin/horcrux-api
docker run -d --name $N-cli --network $N $IMG sleep 3600 >/dev/null
hx_sibling_apt_prep $N-srv $N-cli
docker exec $N-srv bash -c 'for i in 1 2 3 4 5; do apt-get update -qq >/dev/null 2>&1; apt-cache policy nftables | grep -q "Candidate: [0-9]" && break; sleep 5; done; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nftables curl jq libvirt0 iproute2 procps >/dev/null' || { echo "FAIL - apt (server)"; exit 1; }
docker exec $N-cli bash -c 'command -v curl >/dev/null || (apt-get update -qq >/dev/null 2>&1; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq curl >/dev/null)' || { echo "FAIL - apt (client)"; exit 1; }
SIP=$(docker inspect -f '{{(index .NetworkSettings.Networks "'$N'").IPAddress}}' $N-srv)
CIP=$(docker inspect -f '{{(index .NetworkSettings.Networks "'$N'").IPAddress}}' $N-cli)
echo "server $SIP client $CIP"
start_srv() {
  docker exec -d $N-srv bash -c 'mkdir -p /d/log && HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_HTTP_REDIRECT_PORT=0 HORCRUX_DATA_DIR=/d HORCRUX_DATABASE_URL=sqlite:///d/horcrux.db HORCRUX_LOG_DIR=/d/log HORCRUX_AUDIT_LOG=/d/log/audit.log HORCRUX_SSL_DIR=/d/ssl HORCRUX_QEMU_RUN_DIR=/d/run ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 horcrux-api > /d/log/srv.log 2>&1'
  for i in $(seq 1 60); do
    docker exec $N-srv curl -sk -m 3 -o /dev/null https://127.0.0.1:8006/api/health && return 0
    sleep 2
  done
  return 1
}
start_srv && ok "server up" || { bad "server did not start"; docker exec $N-srv tail -20 /d/log/srv.log; cleanup; exit 1; }
tok() { docker exec $N-srv bash -c "curl -sk https://127.0.0.1:8006/api/auth/login -H 'content-type: application/json' -d '{\"username\":\"admin\",\"password\":\"admin\"}' | jq -r '.ticket // .token'"; }
api() { # method path [json]
  docker exec $N-srv bash -c "curl -sk -X $1 https://127.0.0.1:8006/api$2 -H 'authorization: Bearer $TOK' -H 'content-type: application/json' ${3:+-d '$3'}"
}
TOK=$(tok)
[ -n "$TOK" ] && [ "$TOK" != null ] && ok "admin login" || { bad "no token"; cleanup; exit 1; }
SET=$(api GET /security/settings | jq -c '.detection.brute_force.failures=3 | .detection.brute_force.block=true | .detection.brute_force.block_minutes=5')
api PUT /security/settings "$SET" | jq -e '.detection.brute_force.block == true' >/dev/null && ok "blocking enabled in settings" || bad "settings not saved"
# the client cannot be seen by the server as loopback: it comes from $CIP
reach() { docker exec $N-cli curl -sk -m 5 -o /dev/null https://$SIP:8006/api/health; echo $?; }
[ "$(reach)" = 0 ] && ok "client reaches the server before the attack" || bad "client cannot reach the server at the start"
for i in 1 2 3 4; do
  docker exec $N-cli bash -c "curl -sk -m 20 -o /dev/null -w '%{http_code}\n' https://$SIP:8006/api/auth/login -H 'content-type: application/json' -d '{\"username\":\"root\",\"password\":\"wrong$i\"}'" &
done
wait
R=$(api POST /security/activity/check '{}')
echo "check: $R"
echo "$R" | jq -e --arg c "$CIP" '.brute_force==[$c] and .blocked==[$c]' >/dev/null && ok "detector fired and blocked $CIP" || bad "detector result"
docker exec $N-srv nft list table inet horcrux_secblock | tee "$NFTTXT" | grep -q "$CIP" && ok "nft set holds $CIP" || bad "nft set lacks $CIP"
grep -q "timeout 5m" "$NFTTXT" || grep -q "expires" "$NFTTXT" && ok "element carries a timeout" || bad "no timeout on the element"
[ "$(reach)" != 0 ] && ok "client is cut off (curl exit $(reach))" || bad "client still reaches the server"
docker exec $N-srv curl -sk -m 5 -o /dev/null https://127.0.0.1:8006/api/health && ok "loopback still works" || bad "loopback broken"
api GET /security/blocks | jq -e --arg c "$CIP" 'map(.ip)==[$c]' >/dev/null && ok "GET /security/blocks lists it" || bad "block list"
api GET "/security/activity/events?kind=blocked,brute_force" | jq -e '.total>=2' >/dev/null && ok "timeline has the detection and the block" || bad "timeline"
# an unblock restores access
docker exec $N-srv bash -c "curl -sk -o /dev/null -w '%{http_code}' -X DELETE 'https://127.0.0.1:8006/api/security/blocks?ip=$CIP' -H 'authorization: Bearer $TOK'" | grep -q 204 && ok "unblock answered 204" || bad "unblock"
[ "$(reach)" = 0 ] && ok "client reaches the server after unblock" || bad "still cut off after unblock"
# a block survives a restart: block again, restart the server, the empty kernel set is refilled
api POST /security/blocks "{\"ip\":\"$CIP\",\"minutes\":5,\"reason\":\"restart test\"}" | jq -e '.ip' >/dev/null && ok "manual block" || bad "manual block"
docker exec $N-srv pkill -x horcrux-api; sleep 3; docker exec $N-srv pgrep -x horcrux-api >/dev/null && bad "old server still running"
docker exec $N-srv nft delete table inet horcrux_secblock
start_srv && ok "server restarted" || bad "restart"
for i in $(seq 1 15); do docker exec $N-srv nft list table inet horcrux_secblock 2>/dev/null | grep -q "$CIP" && break; sleep 2; done
docker exec $N-srv nft list table inet horcrux_secblock 2>/dev/null | grep -q "$CIP" && ok "block re-applied after the restart" || bad "block not re-applied"
TOK=$(tok); api DELETE "/security/blocks?ip=$CIP" >/dev/null
# unsafe targets are refused
for ip in 127.0.0.1 $SIP; do
  C=$(docker exec $N-srv bash -c "curl -sk -o /dev/null -w '%{http_code}' -X POST https://127.0.0.1:8006/api/security/blocks -H 'authorization: Bearer $TOK' -H 'content-type: application/json' -d '{\"ip\":\"$ip\"}'")
  [ "$C" = 400 ] && ok "refused to block $ip" || bad "block of $ip answered $C"
done
cleanup
[ $fail = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $fail
