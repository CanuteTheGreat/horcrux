#!/bin/bash
# Real Incus end-to-end tests of the LXD/Incus consoles, in a throwaway
# PRIVILEGED sibling container running an Incus daemon (nested, like
# lxc_e2e.sh does for LXC).
#
#   incus_e2e.sh ct <horcrux-api> [api_contract test binary]
#       System containers (no KVM needed): terminal (exec PTY), console
#       (type=console) and console-log WebSockets through the HTTP API
#       (incus_e2e.py MODE=ct), then the UI contract test's Incus part.
#
#   incus_e2e.sh vm <horcrux-api>
#       Incus VMs (needs /dev/kvm): serial console round-trip, SPICE relay
#       handshake (TCP and WebSocket), .vv file (incus_e2e.py MODE=vm).
#
#   incus_e2e.sh serve <horcrux-api> <ui dist dir> <port> [kvm]
#       API + this build's UI (HTTPS) with Incus, in a sibling on the
#       CALLER's docker network (the CI job's network; the default bridge when
#       the caller is not a container) for the browser E2E. Like lxc_e2e.sh,
#       the sibling keeps its own network namespace (nested containers need a
#       fresh sysfs, which a job-owned namespace refuses). Writes the API URL
#       to /tmp/horcrux-incus-e2e.url and the node CA to
#       /tmp/horcrux-incus-e2e-ca.pem. `incus_e2e.sh stop` removes it;
#       `incus_e2e.sh exec <cmd...>` runs a command in it (browser fixtures).
#
# The API serves HTTPS from its node-local CA; every client validates the
# certificate against that CA (/api/tls/ca.pem).
set -euo pipefail
MODE=${1:?usage: $0 ct|vm|serve|stop|exec ...}
HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE=${INCUS_E2E_IMAGE:-ubuntu:24.04}
STATE=/tmp/horcrux-incus-e2e.name
URL_FILE=/tmp/horcrux-incus-e2e.url
CA_OUT=/tmp/horcrux-incus-e2e-ca.pem

cleanup_sibling() {
  local name=$1
  timeout 180 docker exec "$name" bash -c '
    for i in $(incus list -c n --format csv 2>/dev/null </dev/null); do timeout 60 incus delete -f "$i" </dev/null; done
    ip link del incusbr0 2>/dev/null' >/dev/null 2>&1 || true
  docker rm -f "$name" >/dev/null 2>&1 || true
}

case "$MODE" in
  stop)
    if [ -f "$STATE" ]; then cleanup_sibling "$(cat "$STATE")"; rm -f "$STATE"; fi
    rm -f "$URL_FILE" "$CA_OUT"
    exit 0 ;;
  exec)
    shift
    docker exec -i "$(cat "$STATE")" "$@" </dev/null
    exit $? ;;
esac

BIN=${2:?horcrux-api binary required}
NAME="horcrux-incus-e2e-${HX_RUN_TAG:-$$}"
NET_ARGS=()
DEV_ARGS=()
PORT=8006
BIND=127.0.0.1
KVM=0
[ "$MODE" = vm ] && KVM=1
if [ "$MODE" = serve ]; then
  UI_DIST=${3:?ui dist dir required}
  PORT=${4:?port required}
  [ "${5:-}" = kvm ] && KVM=1
  BIND=0.0.0.0
  SELF=$(grep -o '/docker/containers/[0-9a-f]\{64\}' /proc/self/mountinfo | head -1 | cut -d/ -f4 || true)
  if [ -n "$SELF" ]; then
    NET=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$SELF" | awk '{print $1}')
    if [ -n "$NET" ] && [ "$NET" != host ]; then NET_ARGS=(--network "$NET"); fi
  fi
  echo "$NAME" > "$STATE"
else
  trap 'cleanup_sibling "$NAME"' EXIT
fi
if [ "$KVM" = 1 ]; then
  for d in /dev/kvm /dev/vhost-vsock /dev/vhost-net /dev/net/tun; do
    [ -e "$d" ] && DEV_ARGS+=(--device "$d")
  done
  [ -e /dev/kvm ] || { echo "::error::/dev/kvm missing: Incus VMs need the kvm-x86_64 runner" >&2; exit 1; }
fi

docker run -d --init --name "$NAME" --privileged --cgroupns=private --memory 6g --memory-swap 6g \
  "${NET_ARGS[@]}" "${DEV_ARGS[@]}" "$IMAGE" sleep 7200 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$HERE/incus_e2e.py" "$NAME":/tmp/incus_e2e.py
if [ "$MODE" = serve ]; then
  docker cp "$UI_DIST" "$NAME":/srv/horcrux-ui
elif [ -n "${3:-}" ]; then
  docker cp "$3" "$NAME":/usr/local/bin/api_contract
fi

docker exec -e MODE="$MODE" -e PORT="$PORT" -e BIND="$BIND" -e KVM="$KVM" -i "$NAME" bash -s <<'EOS'
set -euo pipefail
# Leave the root cgroup so delegated controllers can be enabled (cgroup2
# forbids processes in a cgroup whose controllers are delegated).
mkdir -p /sys/fs/cgroup/init.scope
for p in $(cat /sys/fs/cgroup/cgroup.procs); do echo "$p" > /sys/fs/cgroup/init.scope/cgroup.procs 2>/dev/null || true; done
sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m 660 /dev/loop$i b 7 $i; done

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
pkgs="incus python3 python3-cryptography curl iproute2 libvirt0"
[ "$KVM" = 1 ] && pkgs="$pkgs qemu-system-x86 ovmf"
apt-get install -y -qq $pkgs >/tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log; exit 1; }
incus --version
# incusd finds incus-agent (put on every VM's config drive, needed for
# `incus exec` into VMs) via PATH; the packaged systemd unit adds this dir.
PATH=/usr/libexec/incus:$PATH nohup /usr/libexec/incus/incusd --group root > /tmp/incusd.log 2>&1 &
incus admin waitready --timeout=120 || { tail -40 /tmp/incusd.log; exit 1; }
incus admin init --minimal </dev/null
incus profile show default </dev/null | head -20

mkdir -p /tmp/incus-e2e-data
UI_ENV=""
if [ -d /srv/horcrux-ui ]; then UI_ENV="HORCRUX_UI_DIR=/srv/horcrux-ui"; fi
# The certificate also names this sibling's own address(es), which is what
# the CI job connects to in serve mode.
SANS=$(hostname -I | tr ' ' ',' | sed 's/,*$//')
cat > /tmp/start-api.sh <<EOF
cd /tmp/incus-e2e-data
env HORCRUX_HOST=$BIND HORCRUX_PORT=$PORT HORCRUX_DATA_DIR=/tmp/incus-e2e-data \\
  HORCRUX_DATABASE_URL=sqlite:///tmp/incus-e2e-data/horcrux.db ADMIN_PASSWORD=admin \\
  HORCRUX_SSL_DIR=/tmp/incus-e2e-data/ssl HORCRUX_TLS_SAN=$SANS \\
  RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 RUST_LOG=info $UI_ENV \\
  nohup horcrux-api >> /tmp/api.log 2>&1 &
echo \$! > /tmp/api.pid
for i in \$(seq 1 60); do
  if curl -sfk https://127.0.0.1:$PORT/api/health >/dev/null; then
    curl -sfk https://127.0.0.1:$PORT/api/tls/ca.pem -o /tmp/horcrux-ca.pem
    # Real certificate validation from here on.
    curl -sf --cacert /tmp/horcrux-ca.pem https://127.0.0.1:$PORT/api/health >/dev/null && exit 0
    echo "certificate does not validate against the node CA"; exit 1
  fi
  sleep 1
done
echo "horcrux-api did not start"; tail -50 /tmp/api.log; exit 1
EOF
bash /tmp/start-api.sh

export HORCRUX_CA_FILE=/tmp/horcrux-ca.pem
case "$MODE" in
  ct)
    API=https://127.0.0.1:$PORT/api MODE=ct python3 /tmp/incus_e2e.py
    if [ -x /usr/local/bin/api_contract ]; then
      echo "[incus-e2e] UI contract test against a real Incus container"
      HORCRUX_CONTRACT_API=https://127.0.0.1:$PORT/api HORCRUX_CONTRACT_INCUS=1 \
        /usr/local/bin/api_contract instance_console_ --nocapture
    fi ;;
  vm)
    API=https://127.0.0.1:$PORT/api MODE=vm python3 /tmp/incus_e2e.py ;;
  serve)
    echo "[incus-e2e] serving the API + UI with Incus on $BIND:$PORT" ;;
esac
EOS

if [ "$MODE" = serve ]; then
  IP=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}' "$NAME" | awk '{print $1}')
  [ -n "$IP" ] || { echo "no address for $NAME"; exit 1; }
  docker exec "$NAME" cat /tmp/horcrux-ca.pem > "$CA_OUT"
  URL="https://$IP:$PORT"
  for i in $(seq 1 30); do
    curl -sf --cacert "$CA_OUT" "$URL/api/health" >/dev/null && break
    [ "$i" = 30 ] && { echo "$URL is not reachable from the caller"; exit 1; }
    sleep 1
  done
  echo "$URL" > "$URL_FILE"
  echo "[incus-e2e] API + UI reachable at $URL"
fi
