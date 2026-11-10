#!/usr/bin/env bash
# apt-get update + install that survives a flaky Ubuntu mirror.
# `apt-get update` exits 0 when an index fetch times out, and the following
# install then fails with "Package 'clang' has no installation candidate"
# (Check job 78450, run 1674: archive.ubuntu.com timed out on the burst runner).
# usage: ci/apt-install.sh <pkg>...   (first package is the candidate probe)
set -u
[ "$#" -gt 0 ] || { echo "usage: $0 pkg..." >&2; exit 2; }
probe="$1"
for attempt in 1 2 3 4 5; do
  sudo apt-get update -qq || true
  if apt-cache policy "$probe" | grep -q 'Candidate: [^(]'; then
    if sudo apt-get install -y "$@"; then exit 0; fi
  fi
  echo "apt-install: attempt $attempt failed (mirror unreachable?), retrying in $((attempt * 15)) s" >&2
  sleep $((attempt * 15))
done
echo "apt-install: giving up after 5 attempts" >&2
exit 100
