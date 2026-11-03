#!/usr/bin/env bash
# tpm-unlock: UNATTENDED boot of a natively encrypted ZFS root. The pool key was sealed to this machine's TPM 2.0 (PCR 7) by the installer;
# this hook unseals it in the initramfs and loads it, so nobody has to type a passphrase at the console.
#   pre-mount 50  tpm-zfskey : tpm2_createprimary + tpm2_load of the sealed blob (/etc/tpm-unlock/key.{pub,priv}) -> tpm2_unseal (PCR 7 policy)
#                              -> `zfs load-key -L file:///run/tpm/key` -> key shredded.
#   Any failure (other TPM, changed PCRs, no TPM): the stock ZFS prompt follows, where the RECOVERY KEY printed at install time is accepted.
check() { return 255; }
depends() { echo zfs; }
installkernel() { instmods tpm tpm_tis tpm_tis_core tpm_crb tpm_tis_spi tpm_tis_i2c =drivers/char/tpm 2>/dev/null || true; }
install() {
    inst_multiple tpm2_createprimary tpm2_load tpm2_unseal tpm2_flushcontext shred mktemp
    # the TCTI libraries tpm2-tools dlopen at run time (not in `ldd`)
    for l in /usr/lib64/libtss2-tcti-device.so* /usr/lib/libtss2-tcti-device.so* /usr/lib64/libtss2-tcti-*.so* ; do [ -e "$l" ] && inst_libdir_file "${l##*/}" 2>/dev/null; done
    inst_simple /etc/tpm-unlock/key.pub
    inst_simple /etc/tpm-unlock/key.priv
    inst_simple /etc/tpm-unlock/pcr
    inst_hook pre-mount 50 "$moddir/tpm-zfskey.sh"
    inst_hook cleanup 10 "$moddir/tpm-cleanup.sh"
}
