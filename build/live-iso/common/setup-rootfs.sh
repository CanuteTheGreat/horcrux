#!/bin/sh
# Final configuration of the live/installer rootfs (runs inside docker build).
set -eu
: "${PROJECT:?}" "${ISO_LABEL:?}" "${PROJECT_TITLE:?}"

chmod +x /etc/init.d/live-* /usr/libexec/live-* /usr/libexec/console-menu-session /usr/bin/live-installer /usr/bin/live-rescue /usr/bin/live-persist 2>/dev/null || true

# --- the stage3 *container* image makes OpenRC think it runs in docker (skips root remount,
# localmount, devfs ...): this rootfs boots on real hardware / KVM
rm -f /.dockerenv
sed -i 's/^#\?rc_sys=.*/rc_sys=""/' /etc/rc.conf

# --- OpenRC runlevels (a stage3 container image ships them empty) ---------
add() { rc-update add "$1" "$2" >/dev/null 2>&1 || echo "warn: rc-update add $1 $2 failed"; }
for s in devfs dmesg udev; do add $s sysinit; done
for s in modules hostname bootmisc sysctl urandom procfs termencoding root localmount swclock; do add $s boot; done
# sshd is deliberately NOT enabled on the live image (the installer enables it on the installed system)
for s in sysklogd cronie dhcpcd iwd local; do add $s default; done
add live-serial default
add live-installer default
# boot-menu modes: forensic rescue (sysinit, before udev), persistence (boot), try-it-live product (default)
add live-forensic sysinit
add live-persist boot
add live-product default

# --- no fstab entries (live: overlay root; installer writes its own) ------
printf '# live system: root is an overlay; the installer writes the real fstab\ntmpfs /tmp tmpfs defaults,nosuid,nodev 0 0\n' > /etc/fstab

# --- identity ---------------------------------------------------------------
echo "${PROJECT}" > /etc/hostname
printf 'hostname="%s"\n' "${PROJECT}" > /etc/conf.d/hostname
# no stock serial getty: the live-serial service owns the console UART (stage3 ships s0:/ttyS0 on x86 and
# f0:/ttyAMA0 on arm64; a second getty on the same UART steals input and prints stray login prompts)
sed -i -E '/^#?[a-z][0-9a-z]*:[0-9]*:respawn:\/sbin\/agetty .*tty(S|AMA|AML|MXC|USB)[0-9]+ /d' /etc/inittab
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
# No default password anywhere: root is LOCKED on the live image (consoles autologin, see
# live-installer service; sshd is off). The installer sets the real password / SSH key.
passwd -l root >/dev/null
sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin prohibit-password/; s/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
grep -q '^PermitRootLogin prohibit-password' /etc/ssh/sshd_config || echo 'PermitRootLogin prohibit-password' >> /etc/ssh/sshd_config
grep -q '^PasswordAuthentication no' /etc/ssh/sshd_config || echo 'PasswordAuthentication no' >> /etc/ssh/sshd_config
mkdir -p /etc/sudoers.d && echo '%wheel ALL=(ALL:ALL) ALL' > /etc/sudoers.d/wheel && chmod 440 /etc/sudoers.d/wheel
for t in ttyS0 ttyS1 ttyAMA0 hvc0; do grep -qx $t /etc/securetty 2>/dev/null || echo $t >> /etc/securetty 2>/dev/null || true; done
# Wi-Fi is handled by iwd (it also does DHCP for wlan), wired NICs by dhcpcd
mkdir -p /etc/iwd && printf '[General]\nEnableNetworkConfiguration=true\n' > /etc/iwd/main.conf
grep -q '^denyinterfaces' /etc/dhcpcd.conf 2>/dev/null || printf '\ndenyinterfaces wlan* wlp*\n' >> /etc/dhcpcd.conf

# --- kernel + live initramfs ------------------------------------------------------
kver=$(ls /lib/modules | sort -V | tail -1)
echo "kernel: $kver"
mkdir -p /etc/dracut.conf.d
cat > /etc/dracut.conf.d/live.conf <<DR
hostonly="no"
hostonly_cmdline="no"
add_dracutmodules+=" dmsquash-live "
omit_dracutmodules+=" plymouth network-manager nfs iscsi biosdevname "
add_drivers+=" squashfs overlay loop iso9660 sr_mod usb_storage uas sd_mod nvme vmd ahci xhci_pci xhci_hcd ehci_pci uhci_hcd ext4 xfs vfat nls_cp437 nls_iso8859_1 virtio_blk virtio_scsi virtio_pci mmc_block sdhci sdhci_pci sdhci_acpi "
compress="gzip"
DR
# root on ZFS (Horcrux): put the zfs dracut module into the generic initramfs when it is installed
if [ -d /usr/lib/dracut/modules.d/90zfs ]; then echo 'add_dracutmodules+=" zfs "' >> /etc/dracut.conf.d/live.conf; fi
dracut -f --no-hostonly --kver "$kver" "/boot/initramfs-${kver}.img" "$kver"
ls -l /boot
ls /boot/vmlinuz-* >/dev/null

# --- slim the image -----------------------------------------------------------------
emerge --quiet --unmerge dev-util/ccache >/dev/null 2>&1 || true
rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost \
       /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* /usr/src/linux* 2>/dev/null || true
find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
