#!/bin/sh
# app-misc/horcrux-test-machine: arm the hardware watchdog and pet it only while the machine is really alive:
# the loop must be scheduled AND a small write+fdatasync to the root filesystem must complete. A hard kernel hang, a
# stuck root-fs I/O or a dead CPU stops the petting and the hardware resets the machine after the driver timeout (30-60 s), so a test laptop
# never needs the owner to power-cycle it. (The NFS server role is untouched: the probe file is on the local root fs.)
PET=${WDT_PET:-5}
DEV=
pick() {
for d in /sys/class/watchdog/watchdog*; do
	[ -d "$d" ] || continue
	case $(cat "$d/identity" 2>/dev/null) in iTCO_wdt*|*iTCO*) DEV=/dev/${d##*/}; break;; esac
done
if [ -z "$DEV" ]; then
	for d in /sys/class/watchdog/watchdog*; do [ -d "$d" ] && { DEV=/dev/${d##*/}; break; }; done
fi
}
pick
# softdog is ALWAYS armed as well: gringotts' iTCO_wdt (Lenovo ThinkBook 16 G6 IRL, TCO_LOCK set) accepts the pets but never
# resets the machine (a SIGSTOPped pet for 7 min and a killed pet for 2 min: no reset), so a wedged userspace/scheduler would
# not recover there. softdog resets the machine when the pet stops; it cannot save a dead CPU, but that needs working hardware.
HW=$DEV
modprobe softdog soft_margin=60 2>/dev/null; sleep 1
SOFT=
for d in /sys/class/watchdog/watchdog*; do
	[ -d "$d" ] || continue
	case $(cat "$d/identity" 2>/dev/null) in "Software Watchdog"*) SOFT=/dev/${d##*/}; break;; esac
done
[ -n "$HW" ] || HW=$SOFT
[ "$SOFT" = "$HW" ] && SOFT=
[ -n "$HW" ] && [ -c "$HW" ] || { echo "horcrux-test-watchdog: no watchdog device" >&2; exit 0; }
STAMP=/var/lib/horcrux-test-watchdog.stamp
exec 3>"$HW" || exit 1
[ -n "$SOFT" ] && [ -c "$SOFT" ] && exec 4>"$SOFT"
trap 'printf V >&3; [ -n "$SOFT" ] && printf V >&4; exec 3>&- 4>&-; exit 0' TERM INT
echo "horcrux-test-watchdog: armed $HW (driver timeout $(cat /sys/class/watchdog/${HW##*/}/timeout 2>/dev/null) s)${SOFT:+ + $SOFT}, pet every ${PET}s" >&2
while :; do
	if timeout -s KILL 15 sh -c "date +%s > $STAMP && sync -d $STAMP" 3>&- 4>&- 2>/dev/null || [ ! -d /var/lib ]; then
		printf . >&3
		[ -n "$SOFT" ] && printf . >&4
	fi
	sleep "$PET" 3>&- 4>&- &   # children must not inherit the watchdog fds (they would keep them open after we exit: restart = EBUSY)
	wait $!
done
