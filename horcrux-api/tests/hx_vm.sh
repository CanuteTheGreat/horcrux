#!/bin/bash
# Boot throwaway Debian VMs (own kernel, KVM when /dev/kvm exists, else TCG)
# for tests that need a real host kernel: CRIU checkpoint/restore of
# containers cannot run inside another container, so the container runtimes
# (LXC, Incus, Docker, Podman) + CRIU live in these VMs.
#
# Source this file, then:
#   hx_vm_prepare                       # tools, image, ssh key, http seed
#   hx_vm_up <name> <ssh-port> [mem-mb] [cpus] [mcast-ip]
#   hx_vm_ssh <name> <command...>       # as root
#   hx_vm_scp <name> <src...> <dst>     # dst is "name:/path"
#   hx_vm_provision <name>              # runtimes + CRIU (hx_vm_provision.sh)
#   hx_vm_down                          # kill every VM of this run
#
# Two VMs talk over a QEMU mcast socket network (second NIC, 10.77.0.x);
# every VM also has user-mode networking for internet access.
HX_VM_DIR=${HX_VM_DIR:-/tmp/hx-vm-$$}
HX_VM_IMG_URL=${HX_VM_IMG_URL:-https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2}
HX_VM_CACHE=${HX_VM_CACHE:-$HOME/.cache/hx-vm}
HX_VM_HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HX_VM_PIDS=()

hx_vm_prepare() {
  mkdir -p "$HX_VM_DIR" "$HX_VM_CACHE"
  if ! command -v qemu-system-x86_64 >/dev/null || ! command -v qemu-img >/dev/null; then
    local SUDO=""; [ "$(id -u)" = 0 ] || SUDO=sudo
    $SUDO apt-get update -qq >/dev/null
    $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq qemu-system-x86 qemu-utils openssh-client curl python3 rsync >/dev/null
  fi
  local img="$HX_VM_CACHE/$(basename "$HX_VM_IMG_URL")"
  if [ ! -s "$img" ]; then curl -fsSL --retry 3 -o "$img.part" "$HX_VM_IMG_URL" && mv "$img.part" "$img"; fi
  [ -s "$img" ] || { echo "cannot fetch $HX_VM_IMG_URL" >&2; return 1; }
  HX_VM_BASE=$img
  [ -f "$HX_VM_DIR/key" ] || ssh-keygen -q -t ed25519 -N '' -f "$HX_VM_DIR/key"
  # cloud-init NoCloud seed over HTTP (10.0.2.2 = the host in QEMU's user net)
  mkdir -p "$HX_VM_DIR/seed"
  cat > "$HX_VM_DIR/seed/user-data" <<EOF
#cloud-config
ssh_pwauth: false
users:
  - name: debian
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $(cat "$HX_VM_DIR/key.pub")
runcmd:
  - mkdir -p /root/.ssh
  - cp /home/debian/.ssh/authorized_keys /root/.ssh/authorized_keys
  - [ systemctl, restart, ssh ]
EOF
  echo "instance-id: hx-$$" > "$HX_VM_DIR/seed/meta-data"
  if [ -z "${HX_VM_SEED_PORT:-}" ]; then
    HX_VM_SEED_PORT=$((20000 + RANDOM % 20000))
    (cd "$HX_VM_DIR/seed" && exec python3 -m http.server "$HX_VM_SEED_PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
    HX_VM_PIDS+=($!)
  fi
}

hx_vm_up() {
  local name=$1 port=$2 mem=${3:-3072} cpus=${4:-2} mcast=${5:-}
  local disk="$HX_VM_DIR/$name.qcow2"
  qemu-img create -q -f qcow2 -b "$HX_VM_BASE" -F qcow2 "$disk" 24G
  local accel=tcg cpu=max
  if [ -w /dev/kvm ]; then accel=kvm; cpu=host; fi
  local nics=(-netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$port-:22" -device virtio-net-pci,netdev=n0)
  if [ -n "$mcast" ]; then
    local mac; mac=$(printf '52:54:00:77:%02x:%02x' $((RANDOM % 256)) $((RANDOM % 256)))
    nics+=(-netdev "socket,id=n1,mcast=230.0.77.1:7777" -device "virtio-net-pci,netdev=n1,mac=$mac")
  fi
  echo "[hx-vm] $name: accel=$accel mem=${mem}M cpus=$cpus ssh=$port"
  qemu-system-x86_64 -machine "q35,accel=$accel" -cpu "$cpu" -m "$mem" -smp "$cpus" -display none -serial "file:$HX_VM_DIR/$name.console" \
    -drive "file=$disk,if=virtio,format=qcow2" \
    -smbios "type=1,serial=ds=nocloud;s=http://10.0.2.2:$HX_VM_SEED_PORT/" \
    "${nics[@]}" >"$HX_VM_DIR/$name.qemu.log" 2>&1 &
  HX_VM_PIDS+=($!)
  echo "$port" > "$HX_VM_DIR/$name.port"
  local i
  for i in $(seq 1 180); do
    if hx_vm_ssh "$name" true 2>/dev/null; then echo "[hx-vm] $name is up after ~$((i * 5)) s"; break; fi
    sleep 5
    if [ "$i" = 180 ]; then echo "[hx-vm] $name did not come up"; tail -30 "$HX_VM_DIR/$name.console"; return 1; fi
  done
  if [ -n "$mcast" ]; then
    hx_vm_ssh "$name" "IF=\$(grep -l '^52:54:00:77:' /sys/class/net/*/address | head -1 | cut -d/ -f5); ip link set \$IF up; ip addr add $mcast/24 dev \$IF"
  fi
}

_hx_ssh_opts() {
  echo "-i $HX_VM_DIR/key -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=5 -o ServerAliveInterval=15"
}

hx_vm_ssh() {
  local name=$1; shift
  ssh $(_hx_ssh_opts) -p "$(cat "$HX_VM_DIR/$name.port")" root@127.0.0.1 "$@"
}

hx_vm_scp() {
  local dst=${@: -1} name
  name=${dst%%:*}
  local srcs=("${@:1:$#-1}")
  scp -q $(_hx_ssh_opts) -P "$(cat "$HX_VM_DIR/$name.port")" "${srcs[@]}" "root@127.0.0.1:${dst#*:}"
}

hx_vm_provision() {
  local name=$1
  hx_vm_scp "$HX_VM_HERE/hx_vm_provision.sh" "$name:/root/hx_vm_provision.sh"
  hx_vm_ssh "$name" "bash /root/hx_vm_provision.sh"
}

# Use a base image with every runtime + CRIU already installed. Built once
# (boot the stock image, hx_vm_provision.sh, power off, flatten) and cached in
# $HX_VM_CACHE keyed by the provisioning script, so later runs only boot it.
hx_vm_use_provisioned() {
  local key; key=$(sha256sum "$HX_VM_HERE/hx_vm_provision.sh" | cut -c1-12)
  local pimg="$HX_VM_CACHE/provisioned-$key.qcow2"
  if [ ! -s "$pimg" ]; then
    echo "[hx-vm] building the provisioned image (one time, ~10 min)"
    hx_vm_up prov "${HX_VM_PROV_PORT:-2299}" 3072 2
    hx_vm_provision prov
    hx_vm_ssh prov "cloud-init clean --logs; sync; systemctl poweroff" || true
    local pid=${HX_VM_PIDS[-1]}
    for i in $(seq 1 60); do kill -0 "$pid" 2>/dev/null || break; sleep 2; done
    kill "$pid" 2>/dev/null || true
    qemu-img convert -O qcow2 -c "$HX_VM_DIR/prov.qcow2" "$pimg.part" && mv "$pimg.part" "$pimg"
    rm -f "$HX_VM_DIR/prov.qcow2"
  fi
  HX_VM_BASE=$pimg
  HX_VM_PROVISIONED=1
}

hx_vm_down() {
  local p
  for p in "${HX_VM_PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  sleep 1
  [ "${KEEP:-0}" = 1 ] || rm -rf "$HX_VM_DIR"
}
