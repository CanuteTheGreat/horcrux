#!/bin/sh
# pre-mount 50, sourced by dracut. Import the pool, unseal the key with the TPM, load it. Never prompts.
tpm_zfskey() {
    [ -s /etc/tpm-unlock/key.priv ] || return 0
    . /lib/dracut-zfs-lib.sh
    decode_root_args || return 0
    ds=$root
    [ "$ds" = "zfs:AUTO" ] && return 0
    pool=${ds%%/*}
    modprobe zfs 2>/dev/null
    for m in tpm_tis tpm_crb; do modprobe $m 2>/dev/null; done
    udevadm settle
    i=0; while [ ! -e /dev/tpmrm0 ] && [ $i -lt 20 ]; do sleep 0.5; i=$((i+1)); done      # /dev/tpmrm0 appears after the kernel's TPM driver probed
    [ -e /dev/tpmrm0 ] || { warn "tpm-unlock: no /dev/tpmrm0 (no TPM 2.0 on this machine?): the ZFS prompt follows"; return 0; }
    if ! zpool get -Ho value name "$pool" >/dev/null 2>&1; then
        # shellcheck disable=SC2086
        zpool import -N $ZPOOL_IMPORT_OPTS "$pool" || { warn "tpm-unlock: pool import failed"; return 0; }
    fi
    er=$(zfs get -Ho value encryptionroot "$ds")
    [ "$er" = "-" ] && return 0
    [ "$(zfs get -Ho value keystatus "$er")" = "unavailable" ] || return 0
    export TPM2TOOLS_TCTI="device:/dev/tpmrm0"
    mkdir -p /run/tpm; chmod 700 /run/tpm
    pcr=$(cat /etc/tpm-unlock/pcr)
    tpm2_createprimary -C o -g sha256 -G ecc -c /run/tpm/prim.ctx >/dev/null 2>&1 \
      && tpm2_load -C /run/tpm/prim.ctx -u /etc/tpm-unlock/key.pub -r /etc/tpm-unlock/key.priv -c /run/tpm/key.ctx >/dev/null 2>&1 \
      && (umask 077; tpm2_unseal -c /run/tpm/key.ctx -p "pcr:$pcr" -o /run/tpm/key >/dev/null 2>&1) \
      || { warn "tpm-unlock: the TPM refused to unseal the disk key (other machine / changed firmware measurements): the ZFS prompt follows (use the recovery key)"; return 0; }
    zfs load-key -L file:///run/tpm/key "$er" && info "tpm-unlock: pool key unsealed by the TPM" || warn "tpm-unlock: zfs load-key failed: the ZFS prompt follows"
}
tpm_zfskey
if [ -e /run/tpm/key ]; then shred -u /run/tpm/key 2>/dev/null || { : > /run/tpm/key; rm -f /run/tpm/key; }; fi
rm -f /run/tpm/prim.ctx /run/tpm/key.ctx
