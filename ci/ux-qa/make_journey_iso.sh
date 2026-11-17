#!/bin/bash
# make_journey_iso.sh OUT.iso - the BIOS-bootable test ISO of user_journey.py: GRUB prints a marker, waits, powers the guest off.
set -e
out=$(readlink -f "$1"); w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
mkdir -p "$w/boot/grub"
printf 'set timeout=0\nterminal_output console\necho "HORCRUX-JOURNEY-ISO-BOOTED"\necho "This is the journey test ISO."\nsleep 8\nhalt\n' > "$w/boot/grub/grub.cfg"
bios=/usr/lib/grub/i386-pc
grub-mkrescue ${bios:+-d $bios} -o "$out" "$w" -- -volid JOURNEY >/dev/null 2>&1
test -s "$out"
