#!/bin/sh
# one prompt: unlock the LUKS2 swap, remember the passphrase (tmpfs, 0600) for hx-zfskey
. /lib/dracut-lib.sh
. /lib/dracut-crypt-lib.sh
dev=/dev/disk/by-uuid/$1
[ -e /dev/mapper/hxswap ] && exit 0
umask 077
mkdir -p /run/hx
ask_for_password --tries 5 --prompt "Disk passphrase (unlocks the system and its hibernation swap)" --tty-echo-off --cmd "/lib/hx-try $dev"
if [ ! -e /dev/mapper/hxswap ]; then
    warn "hx-unlock: the swap was not unlocked: no hibernation resume, the ZFS prompt follows"
    rm -f /run/hx/key "$hookdir"/initqueue/finished/devexists-*hxswap*.sh "$hookdir"/initqueue/finished/devexists-*resume*.sh 2>/dev/null
fi
exit 0
