#!/bin/bash
# Real LXC end-to-end tests (Proxmox pct parity) in a throwaway PRIVILEGED
# sibling container with LXC installed (nested LXC). The sibling gets a
# VLAN-aware bridge, a VLAN 20 gateway interface and dnsmasq for DHCP.
#
#   lxc_e2e.sh test  <horcrux-api> [api_contract test binary]
#       Drive everything through the HTTP API (lxc_e2e.py) and check real
#       effects: config, cgroups inside the container, network, mounts,
#       features, resize, snapshots, clone, PTY terminal and console-log
#       WebSockets. Then run the UI contract test's LXC part against the
#       same server, so the UI's LXC types are checked on real responses.
#
#   lxc_e2e.sh serve <horcrux-api> <ui dist dir> <port>
#       Start the API (serving the web UI, HTTPS) in an LXC sibling on the
#       CALLER's docker network (the CI job's network; the default bridge
#       when the caller is not a container). The sibling gets its own network
#       namespace on purpose: in a namespace owned by the job container the
#       kernel refuses the nested LXC containers a fresh sysfs mount. Writes
#       the API's URL to /tmp/horcrux-lxc-e2e.url and the node CA to
#       /tmp/horcrux-lxc-e2e-ca.pem. `lxc_e2e.sh stop` removes it.
set -euo pipefail
MODE=${1:?usage: $0 test|serve|stop ...}
HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE=${LXC_E2E_IMAGE:-catthehacker/ubuntu:act-latest}
STATE=/tmp/horcrux-lxc-e2e.name
URL_FILE=/tmp/horcrux-lxc-e2e.url
CA_OUT=/tmp/horcrux-lxc-e2e-ca.pem
# Per-run lxcpath: loop device names are global on the runner host, so a
# unique path lets cleanup detach exactly this run's loops.
LXCP=/var/lib/lxc-e2e-$$

cleanup_sibling() {
  local name=$1
  # Stop containers first so their loop devices (shared with the runner
  # host's kernel) are released before the sibling disappears.
  timeout 120 docker exec "$name" bash -c '
    P=$(cat /tmp/lxcpath)
    for c in $(lxc-ls -P "$P" 2>/dev/null); do timeout 30 lxc-stop -P "$P" -n "$c" -k 2>/dev/null; done
    umount "$P"/*/.setup-mnt 2>/dev/null
    losetup -a | grep "($P/" | cut -d: -f1 | xargs -r -n1 losetup -d
    ip link del hxbr0 2>/dev/null; [ -f /tmp/dnsmasq.pid ] && kill "$(cat /tmp/dnsmasq.pid)"' >/dev/null 2>&1 || true
  docker rm -f "$name" >/dev/null 2>&1 || true
}

if [ "$MODE" = stop ]; then
  if [ -f "$STATE" ]; then
    cleanup_sibling "$(cat "$STATE")"
    rm -f "$STATE"
  fi
  rm -f "$URL_FILE" "$CA_OUT"
  exit 0
fi

BIN=${2:?horcrux-api binary required}
NAME="horcrux-lxc-e2e-$$"
NET_ARGS=()
PORT=8006
BIND=127.0.0.1
if [ "$MODE" = serve ]; then
  UI_DIST=${3:?ui dist dir required}
  PORT=${4:?port required}
  BIND=0.0.0.0
  # Join the caller's docker network when the caller is a container (a CI
  # job container), so the job reaches the API at the sibling's address.
  SELF=$(grep -o '/docker/containers/[0-9a-f]\{64\}' /proc/self/mountinfo | head -1 | cut -d/ -f4 || true)
  if [ -n "$SELF" ]; then
    NET=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$SELF" | awk '{print $1}')
    if [ -n "$NET" ] && [ "$NET" != host ]; then NET_ARGS=(--network "$NET"); fi
  fi
  echo "$NAME" > "$STATE"
else
  trap 'cleanup_sibling "$NAME"' EXIT
fi

# --init: a real PID 1 that reaps orphans. Processes started with
# lxc-attach are re-parented to it when their lxc-attach exits; an unreaped
# zombie keeps the container's pid namespace (and `lxc-stop -k`) from ever
# finishing.
docker run -d --init --name "$NAME" --privileged --cgroupns=private --memory 3g --memory-swap 3g \
  "${NET_ARGS[@]}" "$IMAGE" sleep 7200 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$HERE/lxc_e2e.py" "$NAME":/tmp/lxc_e2e.py
if [ "$MODE" = serve ]; then
  docker cp "$UI_DIST" "$NAME":/srv/horcrux-ui
elif [ -n "${3:-}" ]; then
  docker cp "$3" "$NAME":/usr/local/bin/api_contract
fi

# keyctl probe: a static binary printing keyctl()'s errno (0 = allowed), to
# prove the keyctl feature really changes the container's seccomp policy.
PROBE=""
if [ "$MODE" = test ] && command -v gcc >/dev/null 2>&1; then
  cat > /tmp/keyctl_probe.c <<'EOF'
#include <errno.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
int main(void) {
    long r = syscall(SYS_keyctl, 0 /* KEYCTL_GET_KEYRING_ID */, -3 /* session */, 0);
    printf("%d\n", r < 0 ? errno : 0);
    return 0;
}
EOF
  if gcc -static -O2 -o /tmp/keyctl_probe /tmp/keyctl_probe.c 2>/dev/null; then
    docker exec "$NAME" mkdir -p /srv/e2e-bind
    docker cp /tmp/keyctl_probe "$NAME":/srv/e2e-bind/keyctl_probe
    PROBE=/srv/e2e-bind/keyctl_probe
  fi
  [ -n "$PROBE" ] || echo "::warning::gcc -static unavailable: keyctl syscall probe skipped (seccomp file still checked)"
fi

docker exec -e MODE="$MODE" -e PORT="$PORT" -e BIND="$BIND" -e KEYCTL_PROBE="$PROBE" -e LXC_PATH="$LXCP" -i "$NAME" bash -s <<'EOS'
set -euo pipefail
echo "$LXC_PATH" > /tmp/lxcpath
# /dev in a container is a snapshot: loop devices the kernel creates later
# (losetup -f / mount -o loop) have no node here. Pre-create them.
for i in $(seq 0 255); do [ -e /dev/loop$i ] || mknod -m 660 /dev/loop$i b 7 $i; done
# Leave the root cgroup: cgroup2 forbids processes in a cgroup whose
# controllers are delegated. LXC drops a trailing init.scope from its base
# cgroup (as on a systemd host), so containers land in /lxc.payload.<name>.
mkdir -p /sys/fs/cgroup/init.scope
for p in $(cat /sys/fs/cgroup/cgroup.procs); do echo "$p" > /sys/fs/cgroup/init.scope/cgroup.procs 2>/dev/null || true; done
sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq lxc-utils uidmap xz-utils e2fsprogs iproute2 iputils-ping dnsmasq-base curl libvirt0 >/dev/null
lxc-start --version

# Host networking for the containers: VLAN-aware bridge, untagged
# 10.77.0.0/24 with DHCP, and a VLAN 20 gateway 10.20.0.1.
ip link add hxbr0 type bridge vlan_filtering 1
ip addr add 10.77.0.1/24 dev hxbr0
ip link set hxbr0 up
bridge vlan add dev hxbr0 vid 20 self
ip link add link hxbr0 name hxbr0.20 type vlan id 20
ip addr add 10.20.0.1/24 dev hxbr0.20
ip link set hxbr0.20 up
dnsmasq --interface=hxbr0 --bind-interfaces --port=0 --dhcp-range=10.77.0.100,10.77.0.150,1h \
  --pid-file=/tmp/dnsmasq.pid --log-facility=/tmp/dnsmasq.log

mkdir -p /tmp/lxc-e2e-data
UI_ENV=""
if [ -d /srv/horcrux-ui ]; then UI_ENV="HORCRUX_UI_DIR=/srv/horcrux-ui"; fi
# The certificate also names this sibling's own address(es), which is what
# the CI job connects to in serve mode.
SANS=$(hostname -I | tr ' ' ',' | sed 's/,*$//')
cat > /tmp/start-api.sh <<EOF
cd /tmp/lxc-e2e-data
env HORCRUX_LXC_PATH=$LXC_PATH HORCRUX_HOST=$BIND HORCRUX_PORT=$PORT HORCRUX_DATA_DIR=/tmp/lxc-e2e-data \
  HORCRUX_DATABASE_URL=sqlite:///tmp/lxc-e2e-data/horcrux.db ADMIN_PASSWORD=admin \
  HORCRUX_SSL_DIR=/tmp/lxc-e2e-data/ssl HORCRUX_TLS_SAN=$SANS \
  RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 RUST_LOG=info $UI_ENV \
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

if [ "$MODE" = test ]; then
  API=https://127.0.0.1:$PORT/api HORCRUX_CA_FILE=/tmp/horcrux-ca.pem python3 /tmp/lxc_e2e.py
  if [ -x /usr/local/bin/api_contract ]; then
    echo "[lxc-e2e] UI contract test against real LXC responses"
    HORCRUX_CONTRACT_API=https://127.0.0.1:$PORT/api HORCRUX_CONTRACT_LXC=1 \
      /usr/local/bin/api_contract lxc_ --nocapture
  fi
else
  echo "[lxc-e2e] serving the API + UI with LXC on $BIND:$PORT"
fi
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
  echo "[lxc-e2e] API + UI reachable at $URL"
fi
