#!/bin/sh
# app-misc/horcrux-test-machine: persistent kernel log. Follows the kernel ring buffer into KLOG_FILE with BOOT-RELATIVE
# stamps ([ 123.456]) plus a wall-clock marker every KLOG_MARK_SECS. Why not dmesg --time-format iso: it fixes the wall-clock
# offset when it starts, before the clock is stepped (swclock/chrony), so every line carried the stale time of the last clean
# shutdown and the end of a boot could not be dated. A marker line (wall, uptime, boot id) dates any line afterwards:
# wall(line) = marker.wall - marker.up + line.stamp. Settings: /etc/conf.d/horcrux-test-klog (+ .local). OpenRC and systemd.
[ -r /etc/conf.d/horcrux-test-klog ] && . /etc/conf.d/horcrux-test-klog
[ -r /etc/conf.d/horcrux-test-klog.local ] && . /etc/conf.d/horcrux-test-klog.local
: "${KLOG_FILE:=/var/log/kernel-test.log}" "${KLOG_MARK_SECS:=60}" "${KLOG_DMESG_ARGS:=--follow --kernel}"
mark() { printf '== MARK wall=%s up=%s boot_id=%s\n' "$(date -Is)" "$(cut -d' ' -f1 /proc/uptime)" "$(cat /proc/sys/kernel/random/boot_id)" >>"$KLOG_FILE"; }
mark
if command -v stdbuf >/dev/null 2>&1; then
	# shellcheck disable=SC2086
	stdbuf -oL -eL dmesg $KLOG_DMESG_ARGS >>"$KLOG_FILE" 2>&1 &
else
	# shellcheck disable=SC2086
	dmesg $KLOG_DMESG_ARGS >>"$KLOG_FILE" 2>&1 &
fi
pid=$!
trap 'kill "$pid" 2>/dev/null; mark; exit 0' TERM INT
while kill -0 "$pid" 2>/dev/null; do
	sleep "$KLOG_MARK_SECS" &
	wait $!
	mark
done
