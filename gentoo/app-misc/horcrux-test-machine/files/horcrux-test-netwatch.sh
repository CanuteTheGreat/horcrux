#!/bin/bash
# Test-machine connectivity watchdog (Wi-Fi driver reload + wired default-route repair). A Wi-Fi link can stay associated while no traffic flows (Surface Pro 1, 2026-11-04: NM went to
# CONNECTED_SITE and stayed there for 5 hours). When NetworkManager reports no full connectivity for GRACE seconds while a Wi-Fi device is
# connected, reload the Wi-Fi driver module and reconnect, logging every action (logger tag horcrux-netwatch + /var/log/horcrux-netwatch.log).
GRACE=${GRACE:-300}; INTERVAL=${INTERVAL:-30}; COOLDOWN=${COOLDOWN:-900}
LOG=/var/log/horcrux-netwatch.log
say() { echo "$(date -Is) $*" >> "$LOG"; logger -t horcrux-netwatch "$*"; }
bad_since=0; last_fix=0
# Wired must hold the default route. After an abrupt reset NetworkManager can bring a wired NIC up with its address but WITHOUT the routes (the Wi-Fi
# link then silently becomes the default route). Seen twice on the test laptops; a re-activation of the connection restores them. Two consecutive
# sightings (>= INTERVAL s apart) are needed, so a normal DHCP in progress is never interrupted. WIRED_FIX=0 turns it off.
WIRED_FIX=${WIRED_FIX:-1}; wired_bad=""
wired_check() {
	[ "$WIRED_FIX" = 1 ] || return 0
	local dev con seen=""
	for dev in $(nmcli -t -f DEVICE,TYPE,STATE device 2>/dev/null | awk -F: '$2=="ethernet" && $3=="connected"{print $1}'); do
		ip -4 -o addr show dev "$dev" 2>/dev/null | grep -q 'scope global' || continue
		[ -n "$(ip -4 route show default dev "$dev" 2>/dev/null)" ] && continue
		con=$(nmcli -t -g GENERAL.CONNECTION device show "$dev" 2>/dev/null)
		[ -n "$con" ] || continue
		[ "$(nmcli -t -g ipv4.never-default connection show "$con" 2>/dev/null)" = yes ] && continue
		seen="$seen $dev"
		case " $wired_bad " in *" $dev "*)
			say "wired $dev has an address but no default route (second sighting): re-activating '$con'"
			nmcli connection up "$con" >/dev/null 2>&1; sleep 5
			say "after re-activation: default route via $dev: $(ip -4 route show default dev "$dev" 2>/dev/null | head -1)";;
		esac
	done
	wired_bad="$seen"
}
while sleep "$INTERVAL"; do
	wired_check
	state=$(nmcli -t -g CONNECTIVITY general 2>/dev/null)
	[ -n "$state" ] || continue
	now=$(date +%s)
	if [ "$state" = full ]; then bad_since=0; continue; fi
	wl=$(nmcli -t -f DEVICE,TYPE,STATE device 2>/dev/null | awk -F: '$2=="wifi" && $3=="connected"{print $1; exit}')
	[ -n "$wl" ] || { bad_since=0; continue; }          # no Wi-Fi link: that is NetworkManager's job, nothing to reload
	# wired with full connectivity would have made the global state 'full'
	[ "$bad_since" -eq 0 ] && { bad_since=$now; say "connectivity=$state with Wi-Fi $wl connected: watching ($GRACE s)"; }
	[ $((now - bad_since)) -ge "$GRACE" ] || continue
	[ $((now - last_fix)) -ge "$COOLDOWN" ] || continue
	drv=$(basename "$(readlink -f /sys/class/net/$wl/device/driver/module 2>/dev/null)")
	say "no connectivity for $((now - bad_since)) s (state=$state, wifi=$wl, driver=$drv, MemAvailable=$(awk '/MemAvailable/{print $2}' /proc/meminfo) kB): reloading the driver"
	nmcli radio wifi off; sleep 2
	if [ -n "$drv" ]; then
		# dependants first (mwifiex_usb -> mwifiex, iwlmvm -> iwlwifi)
		for m in $(lsmod | awk -v d="$drv" '$1==d{print $4}' | tr ',' ' ') ; do modprobe -r "$m" 2>/dev/null; done
		modprobe -r "$drv" 2>>"$LOG"; sleep 2; modprobe "$drv" 2>>"$LOG"
		[ "$drv" = mwifiex_usb ] && modprobe mwifiex_usb 2>/dev/null
		[ "$drv" = iwlwifi ] && modprobe iwlmvm 2>/dev/null
	fi
	sleep 3; nmcli radio wifi on
	last_fix=$now; bad_since=0
	sleep 30; say "after reload: connectivity=$(nmcli -t -g CONNECTIVITY general 2>/dev/null)"
done
