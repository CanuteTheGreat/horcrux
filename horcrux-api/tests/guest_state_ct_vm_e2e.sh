#!/bin/bash
# Container save state / resume with REAL CRIU for LXC, Incus, Docker and
# Podman: the built horcrux-api runs as root in a throwaway Debian VM (own
# kernel; KVM when the runner has /dev/kvm) that has all four runtimes and
# CRIU, so `criu check` passes and checkpoints really dump and restore. See
# hx_vm.sh and guest_state_ct_vm_e2e.py. Nothing on the runner host is touched.
#
# Usage: horcrux-api/tests/guest_state_ct_vm_e2e.sh <path-to-horcrux-api-binary> [runtimes...]
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary> [lxc incus docker podman]}
shift || true
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/hx_vm.sh"
trap 'hx_vm_down' EXIT
hx_vm_prepare
hx_vm_use_provisioned
hx_vm_up a "${HX_SSH_PORT:-2201}" 3072 2
hx_vm_scp "$BIN" "a:/usr/local/bin/horcrux-api"
hx_vm_scp "$HERE/guest_state_ct_vm_e2e.py" "a:/root/guest_state_ct_vm_e2e.py"
rc=0
hx_vm_ssh a "python3 /root/guest_state_ct_vm_e2e.py $*" || rc=$?
if [ $rc -ne 0 ]; then
  echo "---- VM state"; hx_vm_ssh a "criu check 2>&1 | tail -3; docker ps -a; podman ps -a; incus list; lxc-ls -f" || true
fi
exit $rc
