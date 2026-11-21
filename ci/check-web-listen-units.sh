#!/bin/sh
# Standard ports: the horcrux-api service units must be able to bind 443/80 and must not pin a listening port.
# Rules (owner: "let's stick to standard ports!"; docs/WEB-LISTENERS.md):
#   - every horcrux-api systemd unit names CAP_NET_BIND_SERVICE (AmbientCapabilities; and in CapabilityBoundingSet when one is set)
#   - no unit/init script exports HORCRUX_PORT (ports are configuration: Settings > Web listeners, config.toml [server], conf.d)
#   - an OpenRC script with command_user other than root must set capabilities with cap_net_bind_service
set -u
cd "$(dirname "$0")/.." || exit 1
bad=0
fail() { echo "FAIL: $*" >&2; bad=1; }

units=$(find deploy/systemd gentoo/app-emulation -name 'horcrux-api.service' -o -name 'horcrux.service' 2>/dev/null | sort)
[ -n "$units" ] || { echo "FAIL: no horcrux-api units found" >&2; exit 1; }
for u in $units; do
    grep -q '^AmbientCapabilities=.*CAP_NET_BIND_SERVICE' "$u" || fail "$u: AmbientCapabilities lacks CAP_NET_BIND_SERVICE"
    if grep -q '^CapabilityBoundingSet=' "$u"; then
        grep -q '^CapabilityBoundingSet=.*CAP_NET_BIND_SERVICE' "$u" || fail "$u: CapabilityBoundingSet lacks CAP_NET_BIND_SERVICE"
    fi
    grep -Eq '^Environment=.?HORCRUX_PORT=' "$u" && fail "$u: pins HORCRUX_PORT (ports are configuration)"
done

inits=$(find deploy/openrc gentoo/app-emulation build/live-iso/project/overlay/etc/init.d -type f \( -name 'horcrux' -o -name 'horcrux-api' -o -name 'horcrux-api.initd' \) 2>/dev/null | sort)
for i in $inits; do
    grep -Eq 'HORCRUX_PORT=' "$i" && fail "$i: pins HORCRUX_PORT (ports are configuration)"
    user=$(sed -n 's/^command_user=["]\{0,1\}\([^":]*\).*/\1/p' "$i" | head -1)
    if [ -n "$user" ] && [ "$user" != root ]; then
        grep -q '^capabilities=.*cap_net_bind_service' "$i" || fail "$i: command_user=$user needs capabilities=\"^cap_net_bind_service,...\""
    fi
done

# the shipped defaults must be the standard ports
grep -q 'fn default_https_port() -> u16 {' crates/horcrux-core/src/config.rs &&
  sed -n '/fn default_https_port/,/}/p' crates/horcrux-core/src/config.rs | grep -q '443' || fail "crates/horcrux-core/src/config.rs: default HTTPS port is not 443"
sed -n '/fn default_redirect_port/,/}/p' crates/horcrux-core/src/config.rs | grep -q 'Some(80)' || fail "crates/horcrux-core/src/config.rs: default HTTP redirect port is not 80"

[ "$bad" = 0 ] && echo "web listener units OK ($(echo "$units" | wc -l) systemd units, $(echo "$inits" | wc -l) init scripts)"
exit "$bad"
