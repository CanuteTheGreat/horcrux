#!/usr/bin/env bash
# Real-kernel proof for vm::rxfilter: a router namespace resolves a macvtap
# guest's IPv6 global address only when the device's multicast filter carries
# the guest's solicited-node group. Needs root, netns and macvtap support
# (`modprobe macvtap` on the host); run on a CI/lab host, e.g.
#   docker run --rm --privileged -v $PWD:/src -w /src rust:1-bookworm \
#     bash horcrux-api/tests/macvtap_ndp_e2e.sh
set -euo pipefail
command -v ip >/dev/null || { apt-get update -qq && apt-get install -y -qq iproute2 iputils-ping procps >/dev/null; }
command -v ping >/dev/null || { apt-get update -qq && apt-get install -y -qq iputils-ping procps >/dev/null; }
exec cargo test -p horcrux-vm --lib --features qemu -- --ignored --nocapture netns_e2e
