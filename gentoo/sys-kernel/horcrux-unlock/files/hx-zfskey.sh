#!/bin/sh
# pre-mount 50, sourced by dracut: runs AFTER the resume attempt (pre-mount 10). Import the pool and load its key from the one passphrase.
hx_zfskey() {
    [ -s /run/hx/key ] || return 0
    . /lib/dracut-zfs-lib.sh
    decode_root_args || return 0
    ds=$root
    [ "$ds" = "zfs:AUTO" ] && return 0              # unknown pool before the import: let the stock hook ask
    pool=${ds%%/*}
    modprobe zfs 2>/dev/null
    udevadm settle
    if ! zpool get -Ho value name "$pool" >/dev/null 2>&1; then
        info "ZFS: importing $pool (after the resume attempt)"
        # shellcheck disable=SC2086
        zpool import -N $ZPOOL_IMPORT_OPTS "$pool" || { warn "hx: pool import failed"; return 0; }
    fi
    er=$(zfs get -Ho value encryptionroot "$ds")
    [ "$er" = "-" ] && return 0
    [ "$(zfs get -Ho value keystatus "$er")" = "unavailable" ] || return 0
    zfs load-key -L file:///run/hx/key "$er" || warn "hx: zfs load-key from the unlock passphrase failed: the ZFS prompt follows"
}
hx_zfskey
if [ -e /run/hx/key ]; then shred -u /run/hx/key 2>/dev/null || { : > /run/hx/key; rm -f /run/hx/key; }; fi
