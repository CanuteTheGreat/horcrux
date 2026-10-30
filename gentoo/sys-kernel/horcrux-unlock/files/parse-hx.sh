#!/bin/sh
# kernel arg rd.hx.swap=<UUID of the LUKS2 swap partition>
hxs=$(getarg rd.hx.swap=)
if [ -n "$hxs" ]; then
    {
        printf 'SUBSYSTEM=="block", ACTION=="add|change", ENV{ID_FS_TYPE}=="crypto_LUKS", ENV{ID_FS_UUID}=="%s", ' "$hxs"
        printf 'RUN+="/sbin/initqueue --settled --onetime --unique --name hx-unlock /sbin/hx-unlock %s"\n' "$hxs"
    } >> /etc/udev/rules.d/70-hx-unlock.rules
    wait_for_dev /dev/mapper/hxswap
fi
unset hxs
