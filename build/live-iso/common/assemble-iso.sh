#!/bin/bash
# assemble-iso.sh <rootfs-dir> <out.iso> <volid> <title> <project>
# Builds a UEFI+BIOS hybrid (USB-writable) ISO from an extracted rootfs.
set -euo pipefail
ROOT=$1; OUT=$2; VOLID=$3; TITLE=$4; PROJ=$5
W=$(mktemp -d /work/iso.XXXXXX); ISO=$W/iso; mkdir -p "$ISO"/{LiveOS,boot/grub,EFI/BOOT}
kver=$(ls "$ROOT/lib/modules" | sort -V | tail -1)
cp "$ROOT/boot/vmlinuz-$kver" "$ISO/boot/vmlinuz"
cp "$ROOT/boot/initramfs-$kver.img" "$ISO/boot/initramfs.img"
cp "$ROOT/etc/os-release" "$ISO/boot/os-release"

echo "== squashfs"
mksquashfs "$ROOT" "$ISO/LiveOS/squashfs.img" -comp xz -b 256K -Xbcj x86 -no-progress -noappend 2>&1 | tail -3

CMD="root=live:LABEL=${VOLID} rd.live.image rd.live.overlay.overlayfs=1 rootfstype=auto ro quiet"
cat > "$ISO/boot/grub/grub.cfg" <<CFG
set timeout=5
set default=0
insmod all_video
terminal_input console
terminal_output console
# mirror the menu on COM1 when a UART exists (serial consoles, QEMU tests)
if serial --unit=0 --speed=115200; then
    terminal_input console serial
    terminal_output console serial
fi
search --no-floppy --label --set=root ${VOLID}

menuentry "Install ${TITLE} (guided installer)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 ${PROJ}.installer=1
    initrd /boot/initramfs.img
}
menuentry "Install ${TITLE} - serial console (ttyS0 115200)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 console=tty0 console=ttyS0,115200n8 ${PROJ}.installer=1
    initrd /boot/initramfs.img
}
menuentry "Automated install from an answer file (ERASES the target disk)" {
    linux /boot/vmlinuz ${CMD} loglevel=3 console=tty0 console=ttyS0,115200n8 ${PROJ}.autoinstall=1
    initrd /boot/initramfs.img
}
menuentry "${TITLE} - live system (no install)" {
    linux /boot/vmlinuz ${CMD}
    initrd /boot/initramfs.img
}
menuentry "${TITLE} - live system, serial console (ttyS0)" {
    linux /boot/vmlinuz ${CMD} console=tty0 console=ttyS0,115200n8
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
MODS="serial part_gpt part_msdos fat iso9660 normal linux search search_label search_fs_file configfile all_video terminal gzio echo test"
echo "== grub BIOS"
mkdir -p "$ISO/boot/grub/i386-pc"
grub-mkimage -O i386-pc-eltorito -o "$ISO/boot/grub/i386-pc/eltorito.img" -p /boot/grub -c $W/early.cfg biosdisk $MODS
echo "== grub UEFI"
grub-mkimage -O x86_64-efi -o "$ISO/EFI/BOOT/BOOTX64.EFI" -p /boot/grub -c $W/early.cfg $MODS
dd if=/dev/zero of=$W/efi.img bs=1M count=8 status=none
mkfs.vfat -n EFIBOOT $W/efi.img >/dev/null
mmd -i $W/efi.img ::EFI ::EFI/BOOT
mcopy -i $W/efi.img "$ISO/EFI/BOOT/BOOTX64.EFI" ::EFI/BOOT/BOOTX64.EFI

echo "== xorriso"
xorriso -as mkisofs -iso-level 3 -full-iso9660-filenames -volid "$VOLID" \
  -appid "$TITLE live/installer" -publisher "Errant Solutions" \
  --grub2-mbr /usr/lib/grub/i386-pc/boot_hybrid.img \
  -partition_offset 16 \
  -appended_part_as_gpt \
  -append_partition 2 C12A7328-F81F-11D2-BA4B-00A0C93EC93B $W/efi.img \
  -iso_mbr_part_type EBD0A0A2-B9E5-4433-87C0-68B6B72699C7 \
  -b boot/grub/i386-pc/eltorito.img -no-emul-boot -boot-load-size 4 -boot-info-table --grub2-boot-info \
  -eltorito-alt-boot -e --interval:appended_partition_2:all:: -no-emul-boot \
  -eltorito-catalog boot/grub/boot.cat \
  -o "$OUT" "$ISO" 2>&1 | tail -5
rm -rf "$W"
ls -l "$OUT"
