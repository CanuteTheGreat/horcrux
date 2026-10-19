#!/usr/bin/env bash
# Boot a built Horcrux ISO under QEMU (TCG is fine) and check the live
# system from its serial console: the rootfs init came up with the
# release version, and /dev, /run, /tmp and /var are writable mounts over
# the read-only squashfs root (none of the "Read-only file system" errors
# the live init used to print).
#
# usage: ci/iso-boot-smoke.sh <iso> [expected-version]
set -euo pipefail
ISO=${1:?usage: iso-boot-smoke.sh <iso> [version]}
VERSION=${2:-$(grep '^version = "' "$(dirname "$0")/../Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')}
LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT

PROBE='echo PROBE-START; : > /run/lock/p && : > /tmp/p && : > /var/log/horcrux/p && : > /var/lib/horcrux/p && echo WRITABLE-OK; ls -ld /var/run; grep -E " (/dev|/run|/tmp|/var) " /proc/mounts; echo PROBE-END'
# The shell prompt appears well within this on TCG; the probe is typed
# into the serial console after it.
BOOT_WAIT=${BOOT_WAIT:-150}
( sleep "$BOOT_WAIT"; printf '%s\n' "$PROBE"; sleep 20 ) |
    timeout $((BOOT_WAIT + 60)) qemu-system-x86_64 -m 768 -cdrom "$ISO" -boot d \
        -display none -monitor none -serial stdio -no-reboot > "$LOG" 2>&1 || true

fail() { echo "ISO boot smoke: $*" >&2; echo "--- serial console ---" >&2; tail -80 "$LOG" >&2; exit 1; }
grep -aq "Horcrux Init Starting" "$LOG" || fail "the rootfs init never started"
grep -aq "Version: $VERSION" "$LOG" || fail "banner doesn't show version $VERSION"
if grep -aqE "Read-only file system|could not mount" "$LOG"; then
    fail "the live init hit a read-only path or a failed mount"
fi
grep -aq "WRITABLE-OK" "$LOG" || fail "/run, /tmp or /var is not writable"
for m in /dev /run /tmp /var; do
    grep -aqE "^[^ ]+ $m (devtmpfs|tmpfs) " <(tr -d '\r' < "$LOG") || fail "$m is not a devtmpfs/tmpfs mount"
done
echo "ISO boot smoke: init up (version $VERSION), /dev /run /tmp /var writable - passed"
