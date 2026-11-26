#!/usr/bin/env bash
# System Log page against a real syslog daemon: rsyslogd on its own socket + file (with <PRI>), real `logger`,
# the running horcrux-api (BASE_URL) and a real browser. See system_log_e2e.py.
#   BASE_URL=https://localhost:8006 PYTHON=/tmp/pw/bin/python bash system_log_e2e.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
d="$(mktemp -d /tmp/hxsyslog.XXXXXX)"
trap 'kill "$(cat "$d/pid" 2>/dev/null)" 2>/dev/null || true; rm -rf "$d"' EXIT
cat > "$d/rsyslog.conf" <<CONF
module(load="imuxsock" SysSock.Use="off")
input(type="imuxsock" Socket="$d/log")
global(workDirectory="$d")
template(name="hx" type="string" string="<%PRI%>%TIMESTAMP% %HOSTNAME% %syslogtag%%msg:::sp-if-no-1st-sp%%msg:::drop-last-lf%\n")
action(type="omfile" file="$d/messages" template="hx" fileCreateMode="0644" dirCreateMode="0755")
CONF
rsyslogd -n -f "$d/rsyslog.conf" -i "$d/pid" >"$d/rsyslogd.out" 2>&1 &
for _ in $(seq 1 50); do [ -S "$d/log" ] && break; sleep 0.2; done
[ -S "$d/log" ] || { cat "$d/rsyslogd.out"; echo "rsyslogd did not open its socket"; exit 1; }
chmod 755 "$d"; chmod 666 "$d/log"
: > "$d/messages"; chmod 644 "$d/messages"
HX_SYSLOG_SOCK="$d/log" HX_SYSLOG_FILE="$d/messages" HX_WTMP_FILE="$d/wtmp" \
    "${PYTHON:-python3}" "$here/system_log_e2e.py"
