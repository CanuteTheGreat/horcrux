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
# no hardware watchdog (hogshead, a T2 MacBook, exposes none): the kernel's softdog still resets a machine whose userspace/scheduler
# is wedged (it cannot save a dead CPU, but a dead CPU needs hardware)
if [ -z "$DEV" ]; then modprobe softdog soft_margin=60 2>/dev/null; sleep 1; pick; fi
[ -n "$DEV" ] && [ -c "$DEV" ] || { echo "horcrux-test-watchdog: no watchdog device" >&2; exit 0; }
STAMP=/var/lib/horcrux-test-watchdog.stamp
exec 3>"$DEV" || exit 1
trap 'printf V >&3; exec 3>&-; exit 0' TERM INT
echo "horcrux-test-watchdog: armed $DEV (driver timeout $(cat /sys/class/watchdog/${DEV##*/}/timeout 2>/dev/null) s), pet every ${PET}s" >&2
while :; do
	if timeout -s KILL 15 sh -c "date +%s > $STAMP && sync -d $STAMP" 2>/dev/null || [ ! -d /var/lib ]; then
		printf . >&3
	fi
	sleep "$PET" &
	wait $!
done
