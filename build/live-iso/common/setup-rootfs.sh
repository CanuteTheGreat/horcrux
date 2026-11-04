#!/bin/sh
# Final configuration of the live/installer rootfs (runs inside docker build).
set -eu
: "${PROJECT:?}" "${ISO_LABEL:?}" "${PROJECT_TITLE:?}"

chmod +x /etc/init.d/live-* /etc/init.d/zram-swap /usr/libexec/live-* /usr/libexec/console-menu-session /usr/bin/live-installer /usr/bin/live-rescue /usr/bin/live-persist 2>/dev/null || true

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

# --- time: months-off hardware clocks (a stick or board whose RTC battery is flat, a VM) must end up right ----------------------
# chronyd steps the clock (not slews) at start-up and on the first measurements after the network came up, and writes it to the RTC (rtcsync);
# /etc/dhcpcd.exit-hook (overlay) tells chronyd the link is up so it re-resolves its pool and measures at once.
if [ -f /etc/chrony/chrony.conf ]; then
  grep -q '^makestep' /etc/chrony/chrony.conf || echo 'makestep 1.0 3' >> /etc/chrony/chrony.conf
  grep -q '^rtcsync' /etc/chrony/chrony.conf || echo 'rtcsync' >> /etc/chrony/chrony.conf
fi
chmod 0644 /etc/dhcpcd.exit-hook 2>/dev/null || true

# --- kernel + live initramfs ------------------------------------------------------
kver=$(ls /lib/modules | sort -V | tail -1)
echo "kernel: $kver"
# diagnostics + self-heal (daily failed 2026-11-03: a cached layer had built zfs-kmod for the previous kernel while a newer gentoo-kernel-bin was installed on top,
# `dracut: Failed to find module zfs`): every kernel in /lib/modules that will be booted must have its zfs module, else rebuild zfs-kmod for the newest one
echo "== kernels: $(ls /lib/modules | tr '\n' ' ') | zfs.ko: $(find /lib/modules -name zfs.ko | tr '\n' ' ') | /usr/src: $(ls /usr/src | tr '\n' ' ')"
qlist -Iv sys-kernel/gentoo-kernel-bin sys-fs/zfs-kmod sys-fs/zfs sys-kernel/linux-headers || true
if [ -d /usr/lib/dracut/modules.d/90zfs ] && [ ! -f "/lib/modules/$kver/extra/zfs.ko" ] && [ -z "$(find "/lib/modules/$kver" -name zfs.ko)" ]; then
  echo "== zfs module missing for $kver: rebuilding sys-fs/zfs-kmod against it"
  ln -sfn "/usr/src/linux-$kver" /usr/src/linux
  FEATURES="-ccache" emerge --oneshot --usepkg=n --getbinpkg=n sys-fs/zfs-kmod
  find "/lib/modules/$kver" -name zfs.ko | grep -q .
fi
mkdir -p /etc/dracut.conf.d
# add_drivers may only name modules this kernel HAS: `vmd` (Intel VMD, x86 only) does not exist on arm64 and dracut-install then prints
# `dracut[E]: FAILED` for the whole `-m spl zfs squashfs ...` call while dracut itself still exits 0 (aarch64 run 8771). Names that are neither a
# loadable module nor built in are dropped HERE, per kernel/arch, and logged.
DRIVERS=$(python3 - "$kver" <<'PY'
import re, sys
kver = sys.argv[1]
want = ("squashfs overlay loop iso9660 sr_mod usb_storage uas sd_mod nvme vmd ahci xhci_pci xhci_hcd ehci_pci uhci_hcd ext4 xfs vfat "
        "nls_cp437 nls_iso8859_1 virtio_blk virtio_scsi virtio_pci mmc_block sdhci sdhci_pci sdhci_acpi").split()
have = set()
for fn in ("modules.dep", "modules.builtin"):
    try:
        for line in open(f"/lib/modules/{kver}/{fn}", errors="replace"):
            m = re.match(r"(?:.*/)?([^/:]+?)\.ko(?:\.\w+)?(?::|$)", line.strip())
            if m:
                have.add(m.group(1).replace("-", "_"))
    except OSError:
        pass
keep = [w for w in want if w.replace("-", "_") in have]
sys.stderr.write("initramfs drivers not in kernel %s (dropped): %s\n" % (kver, " ".join(w for w in want if w not in keep) or "none"))
print(" ".join(keep))
PY
)
case "$DRIVERS" in *squashfs*) ;; *) echo "FATAL: kernel $kver has no squashfs module/built-in: the live initramfs cannot mount its root" >&2; exit 1;; esac
cat > /etc/dracut.conf.d/live.conf <<DR
hostonly="no"
hostonly_cmdline="no"
add_dracutmodules+=" dmsquash-live "
omit_dracutmodules+=" plymouth network-manager nfs iscsi biosdevname "
add_drivers+=" $DRIVERS "
compress="gzip"
DR
# root on ZFS (Horcrux, Patronus): put the zfs dracut module into the generic initramfs when it is installed
if [ -d /usr/lib/dracut/modules.d/90zfs ]; then echo 'add_dracutmodules+=" zfs "' >> /etc/dracut.conf.d/live.conf; fi
# dracut exits 0 after `dracut[E]: FAILED: dracut-install ...` (a module it could not find): fail the build on any [E] line, and below on a missing zfs/squashfs in the result
rm -f /tmp/dracut.log
dracut -f --no-hostonly --kver "$kver" "/boot/initramfs-${kver}.img" "$kver" 2>&1 | tee /tmp/dracut.log
if grep -q 'dracut\[E\]' /tmp/dracut.log; then echo "FATAL: dracut reported errors (see above); refusing to build an ISO with a broken initramfs" >&2; exit 1; fi
lsinitrd "/boot/initramfs-${kver}.img" > /tmp/initrd.lst 2>&1
if [ -d /usr/lib/dracut/modules.d/90zfs ] && ! grep -Eq '/(zfs|spl)\.ko' /tmp/initrd.lst; then
  echo "FATAL: the initramfs of $kver has no zfs module although zfs is installed: the installed system could not import its pool" >&2; exit 1
fi
grep -Eq 'squashfs' /tmp/initrd.lst || grep -q '/squashfs\.ko' "/lib/modules/$kver/modules.builtin" || { echo "FATAL: the initramfs of $kver has no squashfs support" >&2; exit 1; }
rm -f /tmp/dracut.log /tmp/initrd.lst
ls -l /boot
ls /boot/vmlinuz-* >/dev/null

# --- installer data (needs the Portage tree: runs BEFORE the slimming below) --------------------------------------
# USE-flag descriptions + installed-package USE/REQUIRED_USE/dependency constraints for the installer's USE page
python3 /tmp/mk-portage-meta.py /usr/share/live-installer/portage-meta.json.xz
rm -rf /var/db/repos/live-local /etc/portage/repos.conf/live-local.conf

# --- slim the image -----------------------------------------------------------------
emerge --quiet --unmerge dev-util/ccache >/dev/null 2>&1 || true
rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost \
       /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true   # /usr/src/linux-<ver> stays: it is the kernel build tree zfs-kmod (and every module) is built against, ~170 MB
find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
# Portage's records must match the image: the strip above removed docs/pyc on purpose, so drop exactly those CONTENTS entries (reviewed list in
# vdb-prune.py, group `core`); every other missing file stays visible and fails the `package-contents-complete` QEMU gate
python3 /tmp/vdb-prune.py --groups core
