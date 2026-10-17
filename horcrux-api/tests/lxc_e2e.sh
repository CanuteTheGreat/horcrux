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
#       Start the API (serving the web UI) in an LXC sibling that shares the
#       CALLER's network namespace, so a browser test in the caller reaches
#       it on 127.0.0.1:<port>. `lxc_e2e.sh stop` removes it.
set -euo pipefail
MODE=${1:?usage: $0 test|serve|stop ...}
HERE=$(cd "$(dirname "$0")" && pwd)
IMAGE=${LXC_E2E_IMAGE:-catthehacker/ubuntu:act-latest}
STATE=/tmp/horcrux-lxc-e2e.name
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
  exit 0
fi

BIN=${2:?horcrux-api binary required}
NAME="horcrux-lxc-e2e-$$"
NET_ARGS=()
PORT=8006
if [ "$MODE" = serve ]; then
  UI_DIST=${3:?ui dist dir required}
  PORT=${4:?port required}
  # Share the caller's network namespace when the caller is a container
  # (a CI job container), so 127.0.0.1:<port> reaches the API.
  SELF=$(grep -o '/docker/containers/[0-9a-f]\{64\}' /proc/self/mountinfo | head -1 | cut -d/ -f4 || true)
  if [ -n "$SELF" ]; then NET_ARGS=(--network "container:$SELF"); fi
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

docker exec -e MODE="$MODE" -e PORT="$PORT" -e KEYCTL_PROBE="$PROBE" -e LXC_PATH="$LXCP" -i "$NAME" bash -s <<'EOS'
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
cat > /tmp/start-api.sh <<EOF
cd /tmp/lxc-e2e-data
env HORCRUX_LXC_PATH=$LXC_PATH HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=$PORT HORCRUX_DATA_DIR=/tmp/lxc-e2e-data \
  HORCRUX_DATABASE_URL=sqlite:///tmp/lxc-e2e-data/horcrux.db ADMIN_PASSWORD=admin \
  RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info $UI_ENV \
  nohup horcrux-api >> /tmp/api.log 2>&1 &
echo \$! > /tmp/api.pid
for i in \$(seq 1 60); do curl -sf http://127.0.0.1:$PORT/api/health >/dev/null && exit 0; sleep 1; done
echo "horcrux-api did not start"; tail -50 /tmp/api.log; exit 1
EOF
bash /tmp/start-api.sh

if [ "$MODE" = test ]; then
  API=http://127.0.0.1:$PORT/api python3 /tmp/lxc_e2e.py
  if [ -x /usr/local/bin/api_contract ]; then
    echo "[lxc-e2e] UI contract test against real LXC responses"
    HORCRUX_CONTRACT_API=http://127.0.0.1:$PORT/api HORCRUX_CONTRACT_LXC=1 \
      /usr/local/bin/api_contract lxc_ --nocapture
  fi
else
  echo "[lxc-e2e] serving the API + UI with LXC on 127.0.0.1:$PORT"
fi
EOS
