#!/bin/bash
# A VM's TPM state is carried, kept and never silently replaced - end to end
# with a real swtpm, real QEMU (TCG) and real tpm2-tools in a throwaway
# sibling container (tpm_state_e2e.py): a libvirt domain is imported (fake
# virsh) and the secret sealed in its TPM before the import is unsealed from
# the imported copy; a domain whose TPM state is gone is refused; a VM whose
# state vanished refuses to start until a new TPM is created explicitly;
# backup/restore, a disk-only snapshot rollback and a Proxmox tpmstate0
# volume carry the state too.
# Usage: horcrux-api/tests/tpm_state_e2e.sh <horcrux-api binary> [image]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/sibling-apt.sh"
BIN=${1:?usage: $0 <horcrux-api binary> [image]}
IMAGE=${2:-catthehacker/ubuntu:act-26.04}
NAME="horcrux-tpm-e2e-${HX_RUN_TAG:-$$}"

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker run -d --name "$NAME" --memory 3g "$IMAGE" sleep 1800 >/dev/null
hx_sibling_apt_prep "$NAME"
docker cp "$BIN" "$NAME":/usr/local/bin/horcrux-api
docker cp "$(dirname "$0")/tpm_state_e2e.py" "$NAME":/tmp/tpm_state_e2e.py
docker exec "$NAME" bash -c 'apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  qemu-system-x86 qemu-system-common qemu-utils iproute2 curl python3 libvirt0 swtpm swtpm-tools tpm2-tools >/dev/null
  command -v swtpm >/dev/null && command -v tpm2_unseal >/dev/null || { echo "swtpm/tpm2-tools not installable here"; exit 1; }'

docker exec "$NAME" bash -c '
set -euo pipefail
mkdir -p /tmp/dev /tmp/lv && cd /tmp/dev
# a stand-in for virsh: the domains come from /tmp/lv/domains/<name>.xml
cat > /tmp/lv/virsh <<"EOV"
#!/bin/sh
shift 2
case "$1" in
  list) ls /tmp/lv/domains | sed "s/\.xml$//" ;;
  domstate) echo "shut off" ;;
  dumpxml) n=""; for a in "$@"; do n=$a; done; cat "/tmp/lv/domains/$n.xml" ;;
  *) exit 0 ;;
esac
EOV
chmod +x /tmp/lv/virsh
cat > /tmp/dev/start-api.sh <<EOS
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR=/tmp/dev \\
HORCRUX_DATABASE_URL=sqlite:///tmp/dev/horcrux.db HORCRUX_SSL_DIR=/tmp/dev/ssl \\
HORCRUX_VM_STORAGE=/tmp/dev/vms ADMIN_PASSWORD=admin RUST_LOG=info \\
HORCRUX_VIRSH=/tmp/lv/virsh HORCRUX_LIBVIRT_SWTPM_DIR=/tmp/lv/swtpm \\
  nohup horcrux-api >> /tmp/dev/api.log 2>&1 &
echo \$! > /tmp/dev/api.pid
for i in \$(seq 1 60); do curl -skf https://127.0.0.1:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "API did not start"; tail -50 /tmp/dev/api.log; exit 1
EOS
bash /tmp/dev/start-api.sh
python3 /tmp/tpm_state_e2e.py || { echo "---- api.log"; tail -80 /tmp/dev/api.log; exit 1; }
'
