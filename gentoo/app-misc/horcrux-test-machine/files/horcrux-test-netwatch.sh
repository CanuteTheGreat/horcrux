#!/bin/bash
# Test-machine network + boot-health watchdog (app-misc/horcrux-test-machine). Settings: /etc/conf.d/horcrux-test-netwatch.
#
# 1. NO NETWORK AT ALL after boot. If NOLINK_SECS after boot no interface except lo is up, the device drivers were never loaded or
#    NetworkManager never came up (a boot where OpenRC skipped udev-trigger left only `lo`, and nobody could reach the machine until
#    someone sat at its console). Coldplug every device (the init system's udev trigger service when it is not started, then
#    udevadm trigger --action=add + settle) and restart NetworkManager. At most once per NOLINK_COOLDOWN.
#    "No link" must be real: a laptop that just resumed from suspend has every link down for a few seconds (Wi-Fi re-associating, USB
#    Ethernet renegotiating), which used to look like a boot with no network and made this script restart NetworkManager after every
#    wake. So the condition has to hold for NOLINK_DEBOUNCE s across consecutive polls, a gap in the polling (suspend) restarts that
#    count, and a link counts as up when NetworkManager reports a connected device or an interface has carrier + a global address.
#    NetworkManager is restarted through the init system (rc-service / systemctl), never by killing it; an instance the init system
#    does not track (left running across a package upgrade that changed the init script) is stopped first and re-started under
#    supervision, otherwise OpenRC marks the service `failed` while the stray daemon keeps the links up.
# 2. BOOT HEALTH. Once, NOLINK_SECS after boot: write HEALTH_FILE (key=value: what the init system thinks the system type is, root rw/ro,
#    links up, boot-runlevel services not started, CRITICAL ones among them, verdict) and log it, so the lab sees a broken boot in
#    minutes instead of a silent laptop.
# 3. WI-FI WITHOUT CONNECTIVITY. A Wi-Fi link can stay associated while no traffic flows (2026-11-04: NetworkManager sat at
#    CONNECTED_SITE for 5 hours). When NetworkManager reports no full connectivity for GRACE seconds while a Wi-Fi device is connected,
#    reload the Wi-Fi driver module and reconnect.
# Every action is logged (logger tag horcrux-netwatch + LOG).
GRACE=300; INTERVAL=30; COOLDOWN=900
NOLINK_SECS=180; NOLINK_COOLDOWN=900; NOLINK_FIX=yes; NOLINK_DEBOUNCE=90; NM_RECONCILE=yes
CRITICAL="devfs udev udev-trigger root localmount NetworkManager sshd"
HEALTH_FILE=/run/horcrux-test-boot-health
LOG=/var/log/horcrux-netwatch.log
[ -r /etc/conf.d/horcrux-test-netwatch ] && . /etc/conf.d/horcrux-test-netwatch
say() { echo "$(date -Is) $*" >> "$LOG" 2>/dev/null; logger -t horcrux-netwatch "$*"; }
uptime_s() { cut -d. -f1 /proc/uptime; }
systemd_booted() { [ -d /run/systemd/system ]; }

links_up() {    # interfaces other than lo that are up (operstate up, or carrier with a global address: USB NICs and some Wi-Fi drivers report "unknown")
	local d n st out=""
	for d in /sys/class/net/*; do
		n=${d##*/}
		[ "$n" = lo ] && continue
		st=$(cat "$d/operstate" 2>/dev/null)
		if [ "$st" != up ]; then
			[ "$st" = unknown ] && [ "$(cat "$d/carrier" 2>/dev/null)" = 1 ] || continue
			ip -o addr show dev "$n" scope global 2>/dev/null | grep -q . || continue
		fi
		out="$out $n"
	done
	echo "${out# }"
}

nm_connected() { # NetworkManager itself says a device is connected
	nmcli -t -f STATE device 2>/dev/null | grep -q '^connected'
}

nm_running() { pgrep -x NetworkManager >/dev/null 2>&1; }

nm_tracked() {   # does the init system think NetworkManager is up (started, or inactive = waiting for a connection)?
	if systemd_booted; then systemctl is-active --quiet NetworkManager.service
	else rc-service NetworkManager status 2>/dev/null | grep -q -E 'status: (started|inactive)'
	fi
}

nm_restart() {   # restart NetworkManager THROUGH the init system; adopt a daemon the init system lost track of
	if systemd_booted; then
		systemctl restart NetworkManager.service >>"$LOG" 2>&1
		return
	fi
	if nm_running && ! nm_tracked; then
		say "NetworkManager is running but OpenRC does not track it (stale daemon from before a package upgrade): clearing the state and re-starting it under OpenRC"
		rc-service NetworkManager zap >>"$LOG" 2>&1
		pkill -TERM -x NetworkManager; local i
		for i in 1 2 3 4 5 6 7 8 9 10; do nm_running || break; sleep 1; done
		nm_running && pkill -KILL -x NetworkManager
		rc-service NetworkManager start >>"$LOG" 2>&1
	else
		rc-service NetworkManager restart >>"$LOG" 2>&1 || rc-service NetworkManager start >>"$LOG" 2>&1
	fi
}

not_started() { # OpenRC: services of the sysinit/boot/default runlevels that are not started; systemd: failed units
	if systemd_booted; then
		systemctl --failed --plain --no-legend 2>/dev/null | awk '{sub(/\.service$/, "", $1); print $1}' | sort -u | tr '\n' ' '
	else
		local lvl
		for lvl in sysinit boot default; do
			rc-status -C "$lvl" 2>/dev/null | awk '!/^Runlevel/ && !/\[ *started/ && NF && $1 != "*" {print $1}'
		done | sort -u | tr '\n' ' '
	fi
}

sys_type() {    # what the init system thinks this machine is; empty = a real host (bare metal or a VM)
	if systemd_booted; then
		systemd-detect-virt --container 2>/dev/null | grep -v '^none$'
	else
		openrc --sys 2>/dev/null
	fi
}

coldplug() {
	say "no network link for ${NOLINK_DEBOUNCE} s (uptime ${1} s, nothing up besides lo): coldplugging devices and restarting NetworkManager"
	if systemd_booted; then
		systemctl start systemd-udev-trigger.service >>"$LOG" 2>&1
	elif ! rc-service udev-trigger status >/dev/null 2>&1; then
		say "udev-trigger is not started: starting it"
		rc-service udev-trigger start >>"$LOG" 2>&1
	fi
	udevadm trigger --type=subsystems --action=add >>"$LOG" 2>&1
	udevadm trigger --type=devices --action=add >>"$LOG" 2>&1
	udevadm settle --timeout=60 >>"$LOG" 2>&1
	nm_restart
	sleep 30
	say "after coldplug: links='$(links_up)' connectivity=$(nmcli -t -g CONNECTIVITY general 2>/dev/null)"
}

boot_health() {
	local ns st root crit="" s verdict=ok
	ns=$(not_started); st=$(sys_type)
	root=$(awk '$2=="/"{split($4,o,","); print o[1]; exit}' /proc/mounts)
	for s in $CRITICAL; do
		case " $ns " in *" $s "*) crit="$crit $s" ;; esac
	done
	crit=${crit# }
	if [ -n "$st" ] || [ "$root" != rw ] || [ -z "$1" ] || [ -n "$crit" ]; then verdict=bad; fi
	{
		echo "verdict=$verdict"
		echo "boot_id=$(cat /proc/sys/kernel/random/boot_id)"
		echo "kernel=$(uname -r)"
		echo "checked_uptime=$(uptime_s)"
		echo "sys_type=$st"
		echo "root=$root"
		echo "links=$1"
		echo "critical_not_started=$crit"
		echo "not_started=${ns% }"
	} > "$HEALTH_FILE.tmp" && mv -f "$HEALTH_FILE.tmp" "$HEALTH_FILE"
	if [ "$verdict" = bad ]; then
		say "BOOT-HEALTH BAD: sys_type='$st' root=$root links='$1' critical_not_started='$crit' not_started='${ns% }'"
	else
		say "boot-health ok: root=rw links='$1' not_started='${ns% }'"
	fi
}

health_done=0; last_nolink=0; bad_since=0; last_fix=0; nolink_since=0; last_tick=$(date +%s); untracked_since=0; last_reconcile=0
while sleep "$INTERVAL"; do
	now=$(date +%s); up=$(uptime_s)
	links=$(links_up)
	# a gap in the polling = the machine was suspended: links need time to come back, so start the "no link" count over
	if [ $((now - last_tick)) -gt $((INTERVAL * 2 + 10)) ]; then nolink_since=0; untracked_since=0; fi
	last_tick=$now
	if [ -n "$links" ] || nm_connected; then nolink_since=0; elif [ "$nolink_since" -eq 0 ]; then nolink_since=$now; fi
	if [ "$nolink_since" -ne 0 ] && [ $((now - nolink_since)) -ge "$NOLINK_DEBOUNCE" ] && [ "$NOLINK_FIX" = yes ] && [ "$up" -ge "$NOLINK_SECS" ] && [ $((now - last_nolink)) -ge "$NOLINK_COOLDOWN" ]; then
		last_nolink=$now; nolink_since=0
		coldplug "$up"
		links=$(links_up)
	fi
	# a NetworkManager the init system lost track of (see header): adopt it once, so the service state matches reality
	if [ "$NM_RECONCILE" = yes ] && ! systemd_booted && [ -n "$links" ] && nm_running && ! nm_tracked; then
		[ "$untracked_since" -eq 0 ] && untracked_since=$now
		if [ $((now - untracked_since)) -ge "$NOLINK_DEBOUNCE" ] && [ $((now - last_reconcile)) -ge "$NOLINK_COOLDOWN" ]; then
			last_reconcile=$now; untracked_since=0
			say "NetworkManager service state is $(rc-service NetworkManager status 2>/dev/null | sed 's/.*status: //') while the daemon runs: re-adopting it under OpenRC"
			nm_restart; sleep 20
			say "after re-adopt: service=$(rc-service NetworkManager status 2>/dev/null | sed 's/.*status: //') links='$(links_up)' connectivity=$(nmcli -t -g CONNECTIVITY general 2>/dev/null)"
		fi
	else untracked_since=0
	fi
	if [ "$health_done" = 0 ] && [ "$up" -ge "$NOLINK_SECS" ]; then
		boot_health "$links"
		health_done=1
	fi
	[ -n "$links" ] || continue
	state=$(nmcli -t -g CONNECTIVITY general 2>/dev/null)
	[ -n "$state" ] || continue
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
