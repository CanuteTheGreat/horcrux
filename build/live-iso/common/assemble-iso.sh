#!/bin/bash
# assemble-iso.sh <rootfs-dir> <out.iso> <volid> <title> <project>
# Builds a UEFI+BIOS hybrid (USB-writable) ISO from an extracted rootfs.
set -euo pipefail
ROOT=$1; OUT=$2; VOLID=$3; TITLE=$4; PROJ=$5
# per-architecture facts (common/arch/<arch>.env, baked into the tools image)
ARCH=${ARCH:-x86_64}
. /usr/local/share/live-arch/$ARCH.env
W=$(mktemp -d /work/iso.XXXXXX); ISO=$W/iso; mkdir -p "$ISO"/{LiveOS,boot/grub,EFI/BOOT}
kver=$(ls "$ROOT/lib/modules" | sort -V | tail -1)
cp "$ROOT/boot/vmlinuz-$kver" "$ISO/boot/vmlinuz"
cp "$ROOT/boot/initramfs-$kver.img" "$ISO/boot/initramfs.img"
cp "$ROOT/etc/os-release" "$ISO/boot/os-release"

echo "== squashfs"
mksquashfs "$ROOT" "$ISO/LiveOS/squashfs.img" -comp xz -b 256K $SQFS_BCJ -no-progress -noappend 2>&1 | tail -3

# x86: mirror the menu on COM1 when a UART exists. Other arches: the firmware's EFI console is already on the UART.
GRUB_SERIAL_BLOCK=""
if [ "$GRUB_SERIAL" = 1 ]; then GRUB_SERIAL_BLOCK='# mirror the menu on COM1 when a UART exists (serial consoles, QEMU tests)
if serial --unit=0 --speed=115200; then
    terminal_input console serial
    terminal_output $gfxout serial
fi
'; fi
# Graphical menu with the product splash (official logo, crop/scale only). Everything is optional: with no
# splash image or no font in the tools image the menu stays the plain text console it always was.
GFX_OK=0
SPLASH="$ROOT/usr/share/live-installer/branding/grub-splash.png"
FONT=$(ls /usr/share/grub/unicode.pf2 /usr/share/grub/*/unicode.pf2 2>/dev/null | head -1 || true)
if [ -f "$SPLASH" ] && [ -n "$FONT" ]; then
  mkdir -p "$ISO/boot/grub/fonts"; cp "$FONT" "$ISO/boot/grub/fonts/unicode.pf2"; cp "$SPLASH" "$ISO/boot/grub/splash.png"; GFX_OK=1
else
  echo "note: no GRUB splash/font (splash=$([ -f "$SPLASH" ] && echo yes || echo no) font=${FONT:-none}); plain text menu"
fi
CMD="root=live:LABEL=${VOLID} rd.live.image rd.live.overlay.overlayfs=1 rootfstype=auto ro quiet"
GFX_BLOCK=""
if [ "$GFX_OK" = 1 ]; then GFX_BLOCK='if loadfont /boot/grub/fonts/unicode.pf2; then
    insmod gfxterm
    insmod png
    set gfxmode=auto
    if terminal_output gfxterm; then
        insmod gfxterm_background
        if background_image /boot/grub/splash.png; then set gfxout=gfxterm; else terminal_output console; fi
    fi
fi
'; fi
cat > "$ISO/boot/grub/grub.cfg" <<CFG
set timeout=5
set default=0
insmod all_video
terminal_input console
terminal_output console
set gfxout=console
${GFX_BLOCK}${GRUB_SERIAL_BLOCK}search --no-floppy --label --set=root ${VOLID}

menuentry "Install ${TITLE} (guided installer)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 ${PROJ}.installer=1
    initrd /boot/initramfs.img
}
menuentry "Install ${TITLE} - serial console (${SERIAL_DEV} 115200)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 console=tty0 console=${SERIAL_DEV},115200n8 ${PROJ}.installer=1
    initrd /boot/initramfs.img
}
menuentry "Automated install from an answer file (ERASES the target disk)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 console=tty0 console=${SERIAL_DEV},115200n8 ${PROJ}.autoinstall=1
    initrd /boot/initramfs.img
}
menuentry "${TITLE} - live system (no install)" {
    linux /boot/vmlinuz ${CMD}
    initrd /boot/initramfs.img
}
menuentry "${TITLE} - live system, serial console (${SERIAL_DEV})" {
    linux /boot/vmlinuz ${CMD} console=tty0 console=${SERIAL_DEV},115200n8
    initrd /boot/initramfs.img
}
menuentry "Install ${TITLE} - safe graphics (nomodeset)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 nomodeset ${PROJ}.installer=1
    initrd /boot/initramfs.img
}
CFG
# tiny early config embedded in the BIOS/UEFI images: find the medium, load the real config
cat > $W/early.cfg <<CFG
search --no-floppy --label --set=root ${VOLID}
set prefix=(\$root)/boot/grub
configfile (\$root)/boot/grub/grub.cfg
CFG
MODS="part_gpt part_msdos fat iso9660 normal linux search search_label search_fs_file configfile all_video terminal gzio echo test font gfxterm gfxterm_background png video"
[ "$GRUB_SERIAL" = 1 ] && MODS="serial $MODS"
if [ "$HAS_BIOS" = 1 ]; then
echo "== grub BIOS"
mkdir -p "$ISO/boot/grub/i386-pc"
grub-mkimage -O i386-pc-eltorito -o "$ISO/boot/grub/i386-pc/eltorito.img" -p /boot/grub -c $W/early.cfg $GRUB_MODS_EXTRA $MODS
fi
echo "== grub UEFI ($EFI_GRUB_TARGET)"
# only embed modules this platform ships (e.g. arm64-efi has no biosdisk)
EMODS=""; for m in $MODS; do if [ -e "/usr/lib/grub/$EFI_GRUB_TARGET/$m.mod" ]; then EMODS="$EMODS $m"; else echo "note: grub $EFI_GRUB_TARGET has no module $m"; fi; done
grub-mkimage -O $EFI_GRUB_TARGET -o "$ISO/EFI/BOOT/$EFI_BOOT_FILE" -p /boot/grub -c $W/early.cfg $EMODS
dd if=/dev/zero of=$W/efi.img bs=1M count=8 status=none
mkfs.vfat -n EFIBOOT $W/efi.img >/dev/null
mmd -i $W/efi.img ::EFI ::EFI/BOOT
mcopy -i $W/efi.img "$ISO/EFI/BOOT/$EFI_BOOT_FILE" ::EFI/BOOT/$EFI_BOOT_FILE

echo "== xorriso"
if [ "$HAS_BIOS" = 1 ]; then
  BIOS_ARGS="--grub2-mbr /usr/lib/grub/i386-pc/boot_hybrid.img -partition_offset 16"
  BIOS_BOOT="-b boot/grub/i386-pc/eltorito.img -no-emul-boot -boot-load-size 4 -boot-info-table --grub2-boot-info -eltorito-alt-boot"
  EFI_PART_ARGS="-append_partition 2 C12A7328-F81F-11D2-BA4B-00A0C93EC93B $W/efi.img -iso_mbr_part_type EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"
else
  # UEFI only: GPT with the ESP as an appended partition (USB stick) + an El Torito EFI image (CD/DVD)
  BIOS_ARGS="-partition_offset 16"; BIOS_BOOT=""
  EFI_PART_ARGS="-append_partition 2 C12A7328-F81F-11D2-BA4B-00A0C93EC93B $W/efi.img"
fi
xorriso -as mkisofs -iso-level 3 -full-iso9660-filenames -volid "$VOLID" \
  -appid "$TITLE live/installer" -publisher "Errant Solutions" \
  $BIOS_ARGS \
  -appended_part_as_gpt \
  $EFI_PART_ARGS \
  $BIOS_BOOT \
  -e --interval:appended_partition_2:all:: -no-emul-boot \
  -eltorito-catalog boot/grub/boot.cat \
  -o "$OUT" "$ISO" 2>&1 | tail -5
rm -rf "$W"
ls -l "$OUT"
