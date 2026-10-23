#!/bin/bash
# Runs INSIDE a Debian (trixie) test VM as root: every container runtime
# Horcrux drives (LXC, Incus, Docker, Podman) plus CRIU, and what the
# horcrux-api binary needs at run time. Prints `criu check` at the end.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq criu lxc incus docker.io podman uidmap xz-utils e2fsprogs iproute2 \
  nftables curl ca-certificates python3 libvirt0 rsync bridge-utils iptables dnsmasq-base \
  qemu-utils skopeo >/dev/null 2>&1 || apt-get install -y criu lxc incus docker.io podman uidmap \
  xz-utils e2fsprogs iproute2 nftables curl ca-certificates python3 libvirt0 rsync bridge-utils iptables dnsmasq-base
mkdir -p /etc/docker
echo '{"experimental": true}' > /etc/docker/daemon.json
systemctl restart docker
systemctl enable --now incus incus.socket 2>/dev/null || true
for i in $(seq 1 30); do incus info >/dev/null 2>&1 && break; sleep 1; done
# Incus: minimal init (dir storage, incusbr0 NAT)
incus admin init --minimal 2>&1 | tail -2 || true
incus network list 2>&1 | head -5 || true
# Docker: pull a small image once so tests don't depend on the registry later
docker pull -q alpine:3.20 >/dev/null || true
podman pull -q docker.io/library/alpine:3.20 >/dev/null || true
echo "--- versions"
criu --version | head -2; incus --version; docker --version; podman --version; lxc-checkpoint --help 2>&1 | head -1 || true
echo "--- criu check"
criu check; echo "criu check rc=$?"
uname -r
