#!/bin/sh
# pkglist.sh FILE... -- the package atoms of these lists for THIS image: comments dropped; a line prefixed `<cond>? ` is kept only when
# <cond> is the build architecture (`uname -m`: x86_64, aarch64, riscv64) or the image's init system ($INIT: openrc, systemd).
# Used by Dockerfile.rootfs (and the variants) so one list serves every arch x init.
INIT=${INIT:-openrc}
A=$(uname -m)
cat "$@" | sed -e 's/#.*//' -e 's/[[:space:]]*$//' | while read -r l; do
	[ -n "$l" ] || continue
	case "$l" in
	*'? '*) c=${l%%\? *}; a=${l#*\? }
		if [ "$c" = "$A" ] || [ "$c" = "$INIT" ]; then echo "$a"; fi ;;
	*) echo "$l" ;;
	esac
done | tr '\n' ' '
