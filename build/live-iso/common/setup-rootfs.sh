#!/bin/sh
# Final configuration of the live/installer rootfs (runs inside docker build).
set -eu
: "${PROJECT:?}" "${ISO_LABEL:?}" "${PROJECT_TITLE:?}"

chmod +x /etc/init.d/live-* /usr/libexec/live-* 2>/dev/null || true

# --- the stage3 *container* image makes OpenRC think it runs in docker (skips root remount,
# localmount, devfs ...): this rootfs boots on real hardware / KVM
rm -f /.dockerenv
sed -i 's/^#\?rc_sys=.*/rc_sys=""/' /etc/rc.conf

# --- OpenRC runlevels (a stage3 container image ships them empty) ---------
add() { rc-update add "$1" "$2" >/dev/null 2>&1 || echo "warn: rc-update add $1 $2 failed"; }
for s in devfs dmesg udev; do add $s sysinit; done
for s in modules hostname bootmisc sysctl urandom procfs termencoding root localmount swclock; do add $s boot; done
for s in sysklogd cronie sshd dhcpcd iwd local; do add $s default; done
add live-serial default
add live-autoinstall default

# --- no fstab entries (live: overlay root; installer writes its own) ------
printf '# live system: root is an overlay; the installer writes the real fstab\ntmpfs /tmp tmpfs defaults,nosuid,nodev 0 0\n' > /etc/fstab

# --- identity ---------------------------------------------------------------
echo "${PROJECT}" > /etc/hostname
printf 'hostname="%s"\n' "${PROJECT}" > /etc/conf.d/hostname
sed -i 's/^#\?s0:.*//' /etc/inittab
cat > /etc/os-release <<OSR
NAME="${PROJECT_TITLE}"
ID=${PROJECT}
ID_LIKE=gentoo
PRETTY_NAME="${PROJECT_TITLE} (Gentoo-based live/installer)"
HOME_URL="https://git.canutethegreat.com/CanuteTheGreat/${PROJECT}"
OSR
echo "${PROJECT}-live" > /etc/live-iso-label
echo "${ISO_LABEL}" > /etc/live-iso-volid

# --- accounts / ssh ------------------------------------------------------------
echo 'root:'"${PROJECT}" | chpasswd
sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin yes/; s/^#\?PasswordAuthentication.*/PasswordAuthentication yes/' /etc/ssh/sshd_config
mkdir -p /etc/sudoers.d && echo '%wheel ALL=(ALL:ALL) ALL' > /etc/sudoers.d/wheel && chmod 440 /etc/sudoers.d/wheel
grep -q ttyS0 /etc/securetty 2>/dev/null || echo ttyS0 >> /etc/securetty 2>/dev/null || true

# --- kernel + live initramfs ------------------------------------------------------
kver=$(ls /lib/modules | sort -V | tail -1)
echo "kernel: $kver"
mkdir -p /etc/dracut.conf.d
cat > /etc/dracut.conf.d/live.conf <<DR
hostonly="no"
hostonly_cmdline="no"
add_dracutmodules+=" dmsquash-live "
omit_dracutmodules+=" plymouth network-manager nfs iscsi biosdevname "
add_drivers+=" squashfs overlay loop iso9660 sr_mod usb_storage uas sd_mod nvme ahci xhci_pci xhci_hcd ehci_pci uhci_hcd ext4 vfat nls_cp437 nls_iso8859_1 virtio_blk virtio_scsi virtio_pci "
compress="gzip"
DR
dracut -f --no-hostonly --kver "$kver" "/boot/initramfs-${kver}.img" "$kver"
ls -l /boot
ls /boot/vmlinuz-* >/dev/null

# --- slim the image -----------------------------------------------------------------
emerge --quiet --unmerge dev-util/ccache >/dev/null 2>&1 || true
rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost \
       /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* /usr/src/linux* 2>/dev/null || true
find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
