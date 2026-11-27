#!/bin/bash
# Real-effect test of Security > Events: real auditd (kernel audit rules) and
# real AIDE in a throwaway Debian VM (audit is not namespaced, so this must
# never run in a container on a shared host). Usage:
#   bash horcrux-api/tests/security_ids_e2e.sh <horcrux-api binary>
# Output lines `ok - ...` / `FAIL: ...`, last line ALL-OK on success.
set -euo pipefail
BIN=${1:?usage: $0 <horcrux-api binary>}
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/hx_vm.sh"
trap 'hx_vm_down' EXIT
hx_vm_prepare
hx_vm_up ids "${HX_SSH_PORT:-2231}" "${HX_VM_MEM:-2048}" 2
hx_vm_ssh ids "export DEBIAN_FRONTEND=noninteractive; for i in 1 2 3; do apt-get update -qq && apt-get install -y -qq auditd aide curl python3 libvirt0 libpam0g >/dev/null && break; sleep 5; done; systemctl start auditd; auditctl -s | head -3; aide --version 2>&1 | head -1"
hx_vm_scp "$BIN" "ids:/usr/local/bin/horcrux-api"
hx_vm_scp "$HERE/security_ids_e2e.py" "ids:/root/security_ids_e2e.py"
hx_vm_ssh ids "python3 /root/security_ids_e2e.py"
