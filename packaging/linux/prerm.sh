#!/bin/sh
# Stop and disable the agent under whichever init is running; tolerate none.
# On upgrade (deb: "upgrade", rpm: $1 = 1) leave it running.
case "${1:-}" in upgrade|1) exit 0 ;; esac
svc=horcrux-guest-agent
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
	systemctl disable --now "$svc.service" >/dev/null 2>&1 || true
fi
if command -v rc-service >/dev/null 2>&1; then
	rc-service "$svc" stop >/dev/null 2>&1 || true
	rc-update del "$svc" >/dev/null 2>&1 || true
fi
exit 0
