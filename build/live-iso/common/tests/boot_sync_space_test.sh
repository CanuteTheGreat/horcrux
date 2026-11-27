#!/bin/bash
# Needs root + loop devices + mkfs.ext4. Reproduces the 2026-11-27 ENOSPC: a /boot full of hardlinked BE snapshots, copied
# link-unaware onto a small ext4 boot copy that is then 100 % full. Proves horcrux-boot-sync's sync_tree repairs it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$here/../overlay/usr/bin/horcrux-boot-sync
T=$(mktemp -d /var/tmp/bootsync-test.XXXXXX)
cleanup() { umount "$T/m" 2>/dev/null || true; [ -n "${LOOP:-}" ] && losetup -d "$LOOP" 2>/dev/null || true; rm -rf "$T"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
mkdir -p "$T/src/be/.objects" "$T/src/be/.snap" "$T/m"
head -c 40M /dev/urandom > "$T/src/be/.objects/k1"; head -c 20M /dev/urandom > "$T/src/be/.objects/k2"
for i in 1 2 3 4 5; do mkdir -p "$T/src/be/.snap/s$i"; ln "$T/src/be/.objects/k1" "$T/src/be/.snap/s$i/vmlinuz"; ln "$T/src/be/.objects/k2" "$T/src/be/.snap/s$i/initramfs"; done
head -c 5M /dev/urandom > "$T/src/vmlinuz-old"
truncate -s 200M "$T/disk"; mkfs.ext4 -q -F "$T/disk"; LOOP=$(losetup -f --show "$T/disk"); mount "$LOOP" "$T/m"
# the old link-unaware sync until the copy is full
rsync -aqx "$T/src/" "$T/m/" 2>/dev/null && fail "fixture should not fit without hardlinks"
dd if=/dev/zero of="$T/m/zz-stale-pad" bs=1M 2>/dev/null || true   # a stale file that fills what rsync left
[ "$(df -B1 --output=avail "$T/m" | tail -1)" -lt 1048576 ] || fail "copy not full"
# master gets a new kernel + a stale file disappears
head -c 6M /dev/urandom > "$T/src/vmlinuz-new"; rm "$T/src/vmlinuz-old"
cp -a "$T/m/vmlinuz-old" "$T/keep.old" 2>/dev/null || true
export HORCRUX_BOOT_SYNC_LIB=1; . "$SCRIPT"
sync_tree "$T/src" "$T/m" || fail "sync_tree failed on the full copy"
rsync "${RSYNC_OPTS[@]}" --delete "$T/src/" "$T/m/"
diff -r "$T/src" "$T/m" -x lost+found >/dev/null || fail "tree differs"
[ "$(stat -c %i "$T/m/be/.snap/s1/vmlinuz")" = "$(stat -c %i "$T/m/be/.objects/k1")" ] || fail "hardlinks not preserved"
# oversize: refused, nothing written
head -c 190M /dev/urandom > "$T/src/huge"
before=$(find "$T/m" -type f | sort | xargs md5sum | md5sum)
if sync_tree "$T/src" "$T/m" 2>/dev/null; then fail "oversize tree was accepted"; fi
[ "$before" = "$(find "$T/m" -type f | sort | xargs md5sum | md5sum)" ] || fail "oversize refusal still modified the copy"
echo PASS
