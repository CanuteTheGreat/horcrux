#!/bin/bash
# Save state on host shutdown / resume on boot for LXC containers (CRIU), for
# real: the built horcrux-api in a throwaway PRIVILEGED sibling container with
# LXC (nested). See guest_state_ct_e2e.py for what is checked. CRIU is
# installed only half way through: first the node WITHOUT it proves the
# fallback (clean shutdown + restart, the reason shown per guest), then with
# it the real checkpoint/restore - or, when the runner's kernel/container
# cannot run CRIU at all, a documented skip of that half (the fallback half
# has already run, and the reason CRIU gives is checked in the API).
#
# Usage: horcrux-api/tests/guest_state_ct_e2e.sh <path-to-horcrux-api-binary> [image]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-latest}
HERE=$(cd "$(dirname "$0")" && pwd)
NAME="horcrux-gsct-e2e-$$"
LXCP=/var/lib/lxc-gsct-$$

cleanup() {
  timeout 120 docker exec "$NAME" bash -c '
    for c in $(lxc-ls -P '"$LXCP"' 2>/dev/null); do timeout 30 lxc-stop -P '"$LXCP"' -n "$c" -k 2>/dev/null; done
    losetup -a | grep "('"$LXCP"'/" | cut -d: -f1 | xargs -r -n1 losetup -d' >/dev/null 2>&1 || true
  [ "${KEEP:-0}" = 1 ] || docker rm -f "$NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --init --name "$NAME" --privileged --cgroupns=private --memory 3g --memory-swap 3g \
  "$IMAGE" sleep 7200 >/dev/null
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$HERE/guest_state_ct_e2e.py" "$NAME":/tmp/guest_state_ct_e2e.py

docker exec -e LXC_PATH="$LXCP" -i "$NAME" bash -s <<'EOS'
set -euo pipefail
for i in $(seq 0 255); do [ -e /dev/loop$i ] || mknod -m 660 /dev/loop$i b 7 $i; done
mkdir -p /sys/fs/cgroup/init.scope
for p in $(cat /sys/fs/cgroup/cgroup.procs); do echo "$p" > /sys/fs/cgroup/init.scope/cgroup.procs 2>/dev/null || true; done
sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
(apt-get install -y -qq lxc-utils || apt-get install -y -qq lxc) >/dev/null; apt-get install -y -qq uidmap xz-utils e2fsprogs iproute2 curl libvirt0 nftables python3 >/dev/null
mkdir -p /tmp/gsct-data
cat > /tmp/env.sh <<EOF
export HORCRUX_LXC_PATH=$LXC_PATH HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/gsct-data
export HORCRUX_DATABASE_URL=sqlite:///tmp/gsct-data/horcrux.db ADMIN_PASSWORD=admin
export HORCRUX_SSL_DIR=/tmp/gsct-data/ssl RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=1000 RUST_LOG=info
export HORCRUX_BOOT_ID_FILE=/tmp/gsct-data/bootid HORCRUX_GUEST_BOOT_DELAY_SECS=3
EOF
cat > /tmp/start-api.sh <<'EOF'
#!/bin/bash
. /tmp/env.sh
if [ -f /tmp/api.pid ]; then kill $(cat /tmp/api.pid) 2>/dev/null || true; sleep 2; fi
cd /tmp/gsct-data
nohup horcrux-api >> /tmp/api.log 2>&1 &
echo $! > /tmp/api.pid
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "horcrux-api did not start"; tail -50 /tmp/api.log; exit 1
EOF
echo boot-1 > /tmp/gsct-data/bootid
bash /tmp/start-api.sh
. /tmp/env.sh
python3 /tmp/guest_state_ct_e2e.py
EOS
