#!/bin/bash
# secureboot-sign.sh: sourced by assemble-iso.sh. Free UEFI Secure Boot for the live ISO and for installed systems:
#
#   firmware (Microsoft UEFI CA in db, as shipped on almost every PC)
#     -> shim (sys-boot/shim = Fedora's shim, signed by Microsoft; includes MokManager)
#       -> our GRUB, signed with OUR key (carries an SBAT section; shim refuses images without one)
#         -> our kernel, signed with OUR key (verified by GRUB through shim's protocol)
#           -> modules: in-tree ones carry the kernel build key, out-of-tree ones (zfs, spl, ...) get ours
#
# The user enrolls our public certificate ONCE in MokManager (ENROLL_THIS_KEY_IN_MOKMANAGER.cer on the medium; shim
# opens MokManager by itself the first time it meets our unknown signature). No paid signing service is involved.
#
# Configuration (environment of assemble-iso.sh):
#   SECUREBOOT=auto|on|off   auto (default): sign when a key is given, else build the unsigned image (old behaviour)
#                            on: a key is required, the build fails without one; off: never sign
#   SECUREBOOT_SIGNING_KEY   the PEM private key itself (CI secret), or
#   SECUREBOOT_KEY_FILE      a path to it (bring your own key)
#   SECUREBOOT_CERT_FILE     the matching certificate (PEM), or SECUREBOOT_CERT = its PEM text; default: the project
#                            certificate shipped in the repo (common/boot/secureboot/signing.crt)
#   SECUREBOOT_SHIM_DIR      where shim + MokManager come from (default: the rootfs' /usr/share/shim, else the tools image's)
# Architectures: x86_64 (shimx64) and aarch64 (shimaa64). riscv64 has no Microsoft-signed shim: never signed.

SB_DIR_IN_ROOT=usr/share/live-installer/secureboot     # what the installer copies to an installed system's ESP
SB_ON=0

sb_log() { echo "secureboot: $*"; }

sb_init() {
    local mode=${SECUREBOOT:-auto}
    case "$mode" in auto|on|off) ;; *) echo "SECUREBOOT must be auto, on or off (got '$mode')" >&2; return 1 ;; esac
    case "$ARCH" in x86_64) SB_S=x64 ;; aarch64) SB_S=aa64 ;; *) SB_S="" ;; esac
    if [ "$mode" = off ]; then sb_log "off (SECUREBOOT=off): unsigned image"; return 0; fi
    if [ -z "$SB_S" ]; then
        [ "$mode" = on ] && { echo "secureboot: $ARCH has no Microsoft-signed shim; SECUREBOOT=on is impossible" >&2; return 1; }
        sb_log "not available on $ARCH (no signed shim exists): unsigned image"; return 0
    fi
    SB_TMP=$(mktemp -d /dev/shm/sb.XXXXXX 2>/dev/null || mktemp -d); chmod 700 "$SB_TMP"
    if [ -n "${SECUREBOOT_SIGNING_KEY:-}" ]; then
        ( umask 077; printf '%s\n' "$SECUREBOOT_SIGNING_KEY" > "$SB_TMP/key.pem" )
    elif [ -n "${SECUREBOOT_KEY_FILE:-}" ]; then
        install -m 600 "$SECUREBOOT_KEY_FILE" "$SB_TMP/key.pem"
    fi
    unset SECUREBOOT_SIGNING_KEY
    if [ ! -s "$SB_TMP/key.pem" ]; then
        rm -rf "$SB_TMP"
        [ "$mode" = on ] && { echo "secureboot: SECUREBOOT=on but no key (SECUREBOOT_SIGNING_KEY / SECUREBOOT_KEY_FILE)" >&2; return 1; }
        sb_log "no signing key given: unsigned image (Secure Boot must be off to boot it)"; return 0
    fi
    local here; here=$(dirname "${BASH_SOURCE[0]}")
    SB_CERT=${SECUREBOOT_CERT_FILE:-}
    if [ -z "$SB_CERT" ] && [ -n "${SECUREBOOT_CERT:-}" ]; then printf '%s\n' "$SECUREBOOT_CERT" > "$SB_TMP/byo.pem"; SB_CERT=$SB_TMP/byo.pem; fi
    [ -n "$SB_CERT" ] || for c in "$here/secureboot/signing.crt" /usr/local/share/live-boot/secureboot/signing.crt; do [ -f "$c" ] && { SB_CERT=$c; break; }; done
    [ -f "$SB_CERT" ] || { echo "secureboot: no certificate (SECUREBOOT_CERT_FILE)" >&2; return 1; }
    cp "$SB_CERT" "$SB_TMP/cert.pem"; SB_CERT=$SB_TMP/cert.pem
    # the key must belong to the certificate users enroll, or every signed file is worthless
    if [ "$(openssl x509 -in "$SB_CERT" -noout -pubkey | sha256sum)" != "$(openssl pkey -in "$SB_TMP/key.pem" -pubout | sha256sum)" ]; then
        echo "secureboot: the signing key does not match $SB_CERT" >&2; return 1
    fi
    openssl x509 -in "$SB_CERT" -outform DER -out "$SB_TMP/cert.der"
    SB_SHIM=${SECUREBOOT_SHIM_DIR:-}
    [ -n "$SB_SHIM" ] || for d in "$ROOT/usr/share/shim" /usr/share/shim; do [ -f "$d/BOOT${SB_S^^}.EFI" ] && { SB_SHIM=$d; break; }; done
    [ -f "$SB_SHIM/BOOT${SB_S^^}.EFI" ] && [ -f "$SB_SHIM/mm$SB_S.efi" ] || { echo "secureboot: no shim (BOOT${SB_S^^}.EFI + mm$SB_S.efi; emerge sys-boot/shim)" >&2; return 1; }
    command -v sbsign >/dev/null || { echo "secureboot: sbsign missing (app-crypt/sbsigntools)" >&2; return 1; }
    # the kernel must be an EFI (PE) image to carry an Authenticode signature (x86 bzImage always is; an arm64 kernel only with
    # its EFI stub / EFI zboot). Checked BEFORE anything is signed, so `auto` falls back to a whole unsigned image.
    local k pe=1
    for k in "$ROOT"/boot/vmlinuz-*; do
        case "$k" in *.old) continue ;; esac
        [ -f "$k" ] && [ "$(head -c 2 "$k")" != "MZ" ] && pe=0
    done
    if [ "$pe" = 0 ]; then
        rm -rf "$SB_TMP"
        [ "$mode" = on ] && { echo "secureboot: the kernel image is not an EFI/PE binary: it cannot be signed" >&2; return 1; }
        echo "::warning::secureboot: the kernel image is not an EFI/PE binary ($ARCH): unsigned image" ; return 0
    fi
    SB_ON=1
    sb_log "ON: signing with $(openssl x509 -in "$SB_CERT" -noout -subject | sed 's/^subject= *//'), sha256 $(openssl x509 -in "$SB_CERT" -noout -fingerprint -sha256 | cut -d= -f2), shim from $SB_SHIM"
}

# sign one PE file in place unless our certificate already verifies it
sb_sign_pe() {
    local f=$1
    sbverify --cert "$SB_CERT" "$f" >/dev/null 2>&1 && return 0
    sbsign --key "$SB_TMP/key.pem" --cert "$SB_CERT" --output "$f.sbtmp" "$f" >/dev/null 2>&1 || { echo "secureboot: sbsign $f failed" >&2; return 1; }
    sbverify --cert "$SB_CERT" "$f.sbtmp" >/dev/null || { echo "secureboot: $f does not verify after signing" >&2; return 1; }
    chmod --reference="$f" "$f.sbtmp"; mv "$f.sbtmp" "$f"
    sb_log "signed $(basename "$f")"
}

# GRUB modules embedded in every signed GRUB image: under Secure Boot GRUB loads no module from disk (lockdown), so all
# a grub.cfg may `insmod` (installer, grub-mkconfig, horcrux-be) must be inside. Missing ones on an arch are skipped.
SB_GRUB_MODS="all_video boot cat chain configfile echo efifwsetup efinet ext2 fat font gfxmenu gfxterm gfxterm_background gzio halt
 iso9660 jpeg loadenv linux lsefi lvm mdraid09 mdraid1x minicmd normal part_apple part_gpt part_msdos png probe reboot regexp search
 search_fs_file search_fs_uuid search_label serial sleep terminal test true video xfs zstd"

sb_grub_image() {   # sb_grub_image <out.efi> <embedded cfg> <prefix>
    local out=$1 cfg=$2 prefix=$3 mods="" m sbat=$SB_TMP/sbat.csv ver
    for m in $SB_GRUB_MODS; do [ -e "/usr/lib/grub/$EFI_GRUB_TARGET/$m.mod" ] && mods="$mods $m"; done
    ver=$(grub-mkimage --version | awk '{print $NF}')
    # SBAT: GRUB upstream's generation line (shipped by sys-boot/grub) + our own vendor line. shim refuses an image whose
    # generation is below its revocation level, and refuses an image with no SBAT section at all.
    if [ -f "/usr/lib/grub/$EFI_GRUB_TARGET/sbat.csv" ]; then cp "/usr/lib/grub/$EFI_GRUB_TARGET/sbat.csv" "$sbat"
    elif [ -f /usr/share/grub/sbat.csv ]; then cp /usr/share/grub/sbat.csv "$sbat"
    else echo "secureboot: GRUB ships no sbat.csv" >&2; return 1; fi
    grep -q "^grub.${PROJ}," "$sbat" || echo "grub.${PROJ},1,Errant Solutions,grub,${ver},https://${PROJ}.errant.solutions/" >> "$sbat"
    grub-mkimage -O "$EFI_GRUB_TARGET" -o "$out" -p "$prefix" -c "$cfg" --sbat "$sbat" $mods
    sb_sign_pe "$out"
}

# kernels + out-of-tree modules inside the rootfs (the installed system copies them), and the installed-system boot files
sb_sign_rootfs() {
    [ "$SB_ON" = 1 ] || return 0
    local k d
    for k in "$ROOT"/boot/vmlinuz-* "$ROOT"/boot/kernel-*; do
        [ -f "$k" ] && [ ! -L "$k" ] || continue
        case "$k" in *.old) continue ;; esac
        sb_sign_pe "$k"
    done
    for d in "$ROOT"/lib/modules/*/ "$ROOT"/usr/lib/modules/*/; do
        [ -d "$d" ] && [ ! -L "${d%/}" ] || continue
        python3 "$(dirname "${BASH_SOURCE[0]}")/sign-modules.py" --key "$SB_TMP/key.pem" --cert "$SB_CERT" "$d" | tail -3
    done
    # the initramfs images were built before signing: their copies of zfs/spl must be signed too or root on ZFS fails to import
    for k in "$ROOT"/boot/initramfs-*.img; do
        [ -f "$k" ] && [ ! -L "$k" ] || continue
        python3 "$(dirname "${BASH_SOURCE[0]}")/resign-initramfs.py" --key "$SB_TMP/key.pem" --cert "$SB_CERT" "$k"
    done
    mkdir -p "$ROOT/$SB_DIR_IN_ROOT"
    # installed systems: GRUB reads the grub.cfg next to itself on the ESP (the installer writes it: it finds /boot by UUID)
    printf 'configfile ${cmdpath}/grub.cfg\n' > "$SB_TMP/installed.cfg"
    sb_grub_image "$ROOT/$SB_DIR_IN_ROOT/grub$SB_S.efi" "$SB_TMP/installed.cfg" /EFI/BOOT
    cp "$SB_SHIM/BOOT${SB_S^^}.EFI" "$ROOT/$SB_DIR_IN_ROOT/shim$SB_S.efi"
    cp "$SB_SHIM/mm$SB_S.efi" "$ROOT/$SB_DIR_IN_ROOT/mm$SB_S.efi"
    cp "$SB_TMP/cert.der" "$ROOT/$SB_DIR_IN_ROOT/ENROLL_THIS_KEY_IN_MOKMANAGER.cer"
    cp "$SB_CERT" "$ROOT/$SB_DIR_IN_ROOT/signing.crt"
    cp "$SB_TMP/sbat.csv" "$ROOT/$SB_DIR_IN_ROOT/sbat.csv"
    printf 'arch=%s\nsuffix=%s\nsigner=%s\nsha256=%s\n' "$ARCH" "$SB_S" "$(openssl x509 -in "$SB_CERT" -noout -subject | sed 's/^subject= *//')" \
        "$(openssl x509 -in "$SB_CERT" -noout -fingerprint -sha256 | cut -d= -f2)" > "$ROOT/$SB_DIR_IN_ROOT/manifest"
    chmod -R a+rX "$ROOT/$SB_DIR_IN_ROOT"
}

# live medium: shim as the removable-media boot file, our GRUB next to it as grub<arch>.efi, MokManager, the certificate
sb_iso_esp() {   # sb_iso_esp <iso tree> <live early.cfg>
    local iso=$1 cfg=$2
    sb_grub_image "$iso/EFI/BOOT/grub$SB_S.efi" "$cfg" /boot/grub
    cp "$SB_SHIM/BOOT${SB_S^^}.EFI" "$iso/EFI/BOOT/$EFI_BOOT_FILE"
    cp "$SB_SHIM/mm$SB_S.efi" "$iso/EFI/BOOT/mm$SB_S.efi"
    cp "$SB_TMP/cert.der" "$iso/ENROLL_THIS_KEY_IN_MOKMANAGER.cer"
    [ -f "$iso/boot/memtest/memtest.efi" ] && sb_sign_pe "$iso/boot/memtest/memtest.efi"
    sbverify --cert "$SB_CERT" "$iso/boot/vmlinuz" >/dev/null || { echo "secureboot: live kernel is not signed" >&2; return 1; }
    echo "Secure Boot: enroll ENROLL_THIS_KEY_IN_MOKMANAGER.cer once in MokManager (docs/ISO-INSTALL.md#secure-boot)" > "$iso/SECURE-BOOT.txt"
}

sb_cleanup() { [ -n "${SB_TMP:-}" ] && rm -rf "$SB_TMP"; SB_TMP=""; }
