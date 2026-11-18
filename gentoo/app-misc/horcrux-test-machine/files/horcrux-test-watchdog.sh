#!/bin/sh
# app-misc/horcrux-test-machine: arm the hardware watchdog and pet it only while the machine is really alive:
# the loop must be scheduled AND a small write+fdatasync to the root filesystem must complete. A hard kernel hang, a
# stuck root-fs I/O or a dead CPU stops the petting and the hardware resets the machine after the driver timeout, so a test laptop
# never needs the owner to power-cycle it. (The NFS server role is untouched: the probe file is on the local root fs.)
# Every value comes from /etc/conf.d/horcrux-test-watchdog (+ .local); nothing is baked in here except the fallbacks below.
WDT_PET=5 WDT_HW_TIMEOUT= WDT_SOFTDOG=yes WDT_SOFT_MARGIN=60 WDT_PROBE_TIMEOUT=15 WDT_DEVICE=auto
for f in /etc/conf.d/horcrux-test-watchdog /etc/conf.d/horcrux-test-watchdog.local; do [ -r "$f" ] && . "$f"; done
PET=${WDT_PET:-5}
DEV=
pick() {
	if [ -n "$WDT_DEVICE" ] && [ "$WDT_DEVICE" != auto ]; then DEV=$WDT_DEVICE; return; fi
	for d in /sys/class/watchdog/watchdog*; do
		[ -d "$d" ] || continue
		case $(cat "$d/identity" 2>/dev/null) in iTCO_wdt*|*iTCO*) DEV=/dev/${d##*/}; break;; esac
	done
	if [ -z "$DEV" ]; then
		for d in /sys/class/watchdog/watchdog*; do
			case $(cat "$d/identity" 2>/dev/null) in "Software Watchdog"*) continue;; esac
			[ -d "$d" ] && { DEV=/dev/${d##*/}; break; }
		done
	fi
}
modprobe iTCO_wdt ${WDT_HW_TIMEOUT:+heartbeat=$WDT_HW_TIMEOUT} 2>/dev/null
sleep 1
pick
# softdog is armed as well by default: some iTCO_wdt units (Lenovo ThinkBook 16 G6 IRL, TCO_LOCK set) accept the pets but never
# reset the machine (a SIGSTOPped pet for 7 min and a killed pet for 2 min: no reset), so a wedged userspace/scheduler would
# not recover there. softdog resets the machine when the pet stops; it cannot save a dead CPU, but that needs working hardware.
HW=$DEV SOFT=
if [ "$WDT_SOFTDOG" = yes ]; then
	modprobe softdog soft_margin="$WDT_SOFT_MARGIN" 2>/dev/null; sleep 1
	for d in /sys/class/watchdog/watchdog*; do
		[ -d "$d" ] || continue
		case $(cat "$d/identity" 2>/dev/null) in "Software Watchdog"*) SOFT=/dev/${d##*/}; break;; esac
	done
fi
[ -n "$HW" ] || HW=$SOFT
[ "$SOFT" = "$HW" ] && SOFT=
[ -n "$HW" ] && [ -c "$HW" ] || { echo "horcrux-test-watchdog: no watchdog device" >&2; exit 0; }
STAMP=/var/lib/horcrux-test-watchdog.stamp
exec 3>"$HW" || exit 1
[ -n "$SOFT" ] && [ -c "$SOFT" ] && exec 4>"$SOFT"
trap 'printf V >&3; [ -n "$SOFT" ] && printf V >&4; exec 3>&- 4>&-; exit 0' TERM INT
# WDIOC_SETTIMEOUT on the open fds (works when the driver was already loaded with another default); needs python3, skipped without it
settimeout() {	# fd seconds
	[ -n "$2" ] || return 0
	python3 -c 'import fcntl,array,sys; fcntl.ioctl(int(sys.argv[1]),0xC0045706,array.array("I",[int(sys.argv[2])]))' "$1" "$2" 2>/dev/null
}
settimeout 3 "$WDT_HW_TIMEOUT" 3>&3 4>&-
[ -n "$SOFT" ] && settimeout 4 "$WDT_SOFT_MARGIN" 3>&- 4>&4
echo "horcrux-test-watchdog: armed $HW (driver timeout $(cat /sys/class/watchdog/${HW##*/}/timeout 2>/dev/null) s)${SOFT:+ + $SOFT (timeout $(cat /sys/class/watchdog/${SOFT##*/}/timeout 2>/dev/null) s)}, pet every ${PET}s" >&2
while :; do
	if timeout -s KILL "$WDT_PROBE_TIMEOUT" sh -c "date +%s > $STAMP && sync -d $STAMP" 3>&- 4>&- 2>/dev/null || [ ! -d /var/lib ]; then
		printf . >&3
		[ -n "$SOFT" ] && printf . >&4
	fi
	sleep "$PET" 3>&- 4>&- &   # children must not inherit the watchdog fds (they would keep them open after we exit: restart = EBUSY)
	wait $!
done
