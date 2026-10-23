#!/bin/bash
# Cross-node container moves (drain / rebalance) with REAL CRIU, for LXC,
# Incus, Docker and Podman: two throwaway Debian VMs (KVM when available)
# joined by a private network, each running the built horcrux-api and all four
# runtimes, clustered. See hx_vm.sh and drain_ct_vm_e2e.py for what is checked.
#
# Usage: horcrux-api/tests/drain_ct_vm_e2e.sh <path-to-horcrux-api-binary> [runtimes...]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [lxc incus docker podman]}
shift || true
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/hx_vm.sh"
trap 'hx_vm_down' EXIT
hx_vm_prepare
hx_vm_use_provisioned
hx_vm_up a "${HX_SSH_PORT_A:-2201}" 2560 2 10.77.0.1
hx_vm_up b "${HX_SSH_PORT_B:-2202}" 2560 2 10.77.0.2
JWT=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
cat > "$HX_VM_DIR/hx-start-api.sh" <<'EOS'
#!/bin/bash
# (re)start this node's horcrux-api; extra environment in /var/lib/hx/extra-env
mkdir -p /var/lib/hx
if [ -f /var/lib/hx/api.pid ]; then kill "$(cat /var/lib/hx/api.pid)" 2>/dev/null || true; sleep 2; fi
pkill -9 -x horcrux-api 2>/dev/null || true
cd /var/lib/hx
env HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/var/lib/hx \
  HORCRUX_DATABASE_URL=sqlite:///var/lib/hx/horcrux.db HORCRUX_SSL_DIR=/var/lib/hx/ssl \
  ADMIN_PASSWORD=admin JWT_SECRET="$(cat /root/jwt.secret)" RATE_LIMIT_MAX_REQUESTS=100000 \
  AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info HORCRUX_BOOT_ID_FILE=/var/lib/hx/bootid \
  HORCRUX_GUEST_BOOT_DELAY_SECS=3 HORCRUX_REBALANCE_TICK_SECS=5 \
  $(cat /var/lib/hx/extra-env 2>/dev/null) \
  setsid nohup horcrux-api >> /var/lib/hx/api.log 2>&1 < /dev/null &
echo $! > /var/lib/hx/api.pid
for i in $(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "horcrux-api did not start"; tail -50 /var/lib/hx/api.log; exit 1
EOS
for v in a b; do
  n=node1; [ $v = b ] && n=node2
  hx_vm_ssh $v "hostnamectl set-hostname $n; grep -q ' $n\$' /etc/hosts || echo '127.0.1.1 $n' >> /etc/hosts; mkdir -p /var/lib/hx; echo boot-1 > /var/lib/hx/bootid; pkill -x horcrux-api || true"
  hx_vm_scp "$BIN" "$v:/usr/local/bin/horcrux-api"
  hx_vm_scp "$HX_VM_DIR/hx-start-api.sh" "$v:/usr/local/bin/hx-start-api.sh"
  hx_vm_ssh $v "chmod +x /usr/local/bin/horcrux-api /usr/local/bin/hx-start-api.sh; echo $JWT > /root/jwt.secret"
done
hx_vm_scp "$HERE/drain_ct_vm_e2e.py" "a:/root/drain_ct_vm_e2e.py"
hx_vm_ssh a "mkdir -p /root/.ssh"
hx_vm_scp "$HX_VM_DIR/key" "a:/root/.ssh/hxvm"
hx_vm_ssh a "chmod 600 /root/.ssh/hxvm"
for v in a b; do hx_vm_ssh $v "bash /usr/local/bin/hx-start-api.sh"; done
rc=0
hx_vm_ssh a "python3 /root/drain_ct_vm_e2e.py $*" || rc=$?
if [ $rc -ne 0 ]; then
  for v in a b; do echo "---- $v api.log"; hx_vm_ssh $v "tail -80 /var/lib/hx/api.log | cut -c1-300" || true; done
fi
exit $rc
