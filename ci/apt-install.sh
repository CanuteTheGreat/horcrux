#!/usr/bin/env bash
# apt-get update + install that survives a flaky Ubuntu mirror.
# `apt-get update` exits 0 when an index fetch times out, and the following
# install then fails with "Package 'clang' has no installation candidate"
# (Check job 78450, run 1674: archive.ubuntu.com timed out on the burst runner).
# usage: ci/apt-install.sh <pkg>...   (first package is the candidate probe)
set -u
[ "$#" -gt 0 ] || { echo "usage: $0 pkg..." >&2; exit 2; }
probe="$1"
# Bound a stalled mirror (runs 4056/4051: connects timing out ate 11+ min per apt call).
APT_OPTS="-o Acquire::Retries=5 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20"
# The runner image carries Launchpad PPAs we never install from; their 404/503s
# make `apt-get update` fail (Ubuntu 26.04 image, git-core PPA, 2026-11-17).
sudo rm -f /etc/apt/sources.list.d/*ppa* 2>/dev/null || true
for attempt in 1 2 3 4 5; do
  sudo timeout 180 apt-get update -qq $APT_OPTS || true
  if apt-cache policy "$probe" | grep -q 'Candidate: [^(]'; then
    if sudo timeout 600 apt-get install -y $APT_OPTS "$@"; then exit 0; fi
  fi
  echo "apt-install: attempt $attempt failed (mirror unreachable?), retrying in $((attempt * 15)) s" >&2
  sleep $((attempt * 15))
done
echo "apt-install: giving up after 5 attempts" >&2
exit 100
