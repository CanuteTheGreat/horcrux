#!/bin/sh
# Final configuration of the live/installer rootfs (runs inside docker build).
set -eu
: "${PROJECT:?}" "${ISO_LABEL:?}" "${PROJECT_TITLE:?}"

chmod +x /etc/init.d/live-* /etc/init.d/zram-swap /usr/libexec/live-* /usr/libexec/console-menu-session /usr/bin/live-installer /usr/bin/live-rescue /usr/bin/live-persist 2>/dev/null || true

# --- init system: the image's INIT (build.sh INIT=openrc|systemd -> the gentoo/stage3 flavour). One implementation of each live service: the
# /etc/init.d script; on systemd its unit (/usr/lib/systemd/system/live-*.service) runs the same script through /usr/libexec/live-openrc-compat.
INIT=${INIT:-openrc}
case "$INIT" in openrc|systemd) ;; *) echo "setup-rootfs: unknown INIT=$INIT" >&2; exit 1 ;; esac
if [ "$INIT" = systemd ]; then
  [ -x /usr/lib/systemd/systemd ] || { echo "FATAL: INIT=systemd but this rootfs has no systemd (stage3 flavour?)" >&2; exit 1; }
fi
rm -f /.dockerenv

if [ "$INIT" = openrc ]; then
# --- the stage3 *container* image makes OpenRC think it runs in docker (skips root remount,
# localmount, devfs ...): this rootfs boots on real hardware / KVM
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
# the systemd twins are not used on this image
rm -f /usr/lib/systemd/system/live-*.service /usr/lib/systemd/system/zram-swap.service; rm -rf /etc/systemd/system/getty@tty1.service.d/00-live.conf
rmdir /etc/systemd/system/getty@tty1.service.d /etc/systemd/system 2>/dev/null || true
else
# --- systemd: same services, same order, as units (journald replaces sysklogd; dhcpcd + iwd stay the network stack on both inits, so the
# installer's network configuration is identical). sshd is deliberately NOT enabled on the live image.
systemctl set-default multi-user.target
systemctl enable getty@tty1.service cronie.service dhcpcd.service iwd.service \
    live-serial.service live-installer.service live-forensic.service live-persist.service live-product.service
# one getty per UART: live-serial owns the console= UART (the systemd-getty-generator would add serial-getty@<console> as well, which
# steals input and prints stray login prompts: the OpenRC image deletes the stage3's s0:/f0: inittab lines for the same reason)
systemctl mask serial-getty@.service
# sshd off (the installer enables it on the installed system); no first-boot wizard on a live boot
systemctl disable sshd.service sshd.socket 2>/dev/null || true
systemctl mask systemd-firstboot.service
# every boot (and every installed system) gets its own machine id: an EMPTY /etc/machine-id makes systemd generate one at boot
: > /etc/machine-id; rm -f /var/lib/dbus/machine-id
# journald: keep the journal in RAM on the live medium (the installed system has /var/log/journal from the package)
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\n# live/installer image default (the installer removes this file)\nStorage=volatile\nRuntimeMaxUse=64M\n' > /etc/systemd/journald.conf.d/50-live.conf
# the OpenRC scripts stay: they ARE the implementation the units run (live-openrc-compat)
fi

# --- no fstab entries (live: overlay root; installer writes its own) ------
printf '# live system: root is an overlay; the installer writes the real fstab\ntmpfs /tmp tmpfs defaults,nosuid,nodev 0 0\n' > /etc/fstab

# --- identity ---------------------------------------------------------------
echo "${PROJECT}" > /etc/hostname
printf 'hostname="%s"\n' "${PROJECT}" > /etc/conf.d/hostname
# no stock serial getty: the live-serial service owns the console UART (stage3 ships s0:/ttyS0 on x86 and
# f0:/ttyAMA0 on arm64; a second getty on the same UART steals input and prints stray login prompts)
[ ! -f /etc/inittab ] || sed -i -E '/^#?[a-z][0-9a-z]*:[0-9]*:respawn:\/sbin\/agetty .*tty(S|AMA|AML|MXC|USB)[0-9]+ /d' /etc/inittab
# /usr/lib/os-release: Horcrux's is owned by sys-apps/horcrux-release (installed by the core layer / the machine meta packages; name, version, build id, variant).
# Products without such a package yet (Patronus) get this installed-system file written here: never "live/installer" (the live session marks itself at boot,
# /usr/libexec/live-os-release); the URLs are the product's pages on the Errant Solutions site (never the Forgejo).
if [ "${PROJECT}" != "horcrux" ]; then
cat > /usr/lib/os-release <<OSR
NAME="${PROJECT_TITLE}"
ID=${PROJECT}
ID_LIKE=gentoo
PRETTY_NAME="${PROJECT_TITLE}"
$( [ "${PROJECT}" = patronus ] && printf 'HOME_URL="https://patronus.errant.solutions/"\nDOCUMENTATION_URL="https://patronus.errant.solutions/docs/"' )
OSR
fi
echo "${PROJECT}-live" > /etc/live-iso-label
echo "${ISO_LABEL}" > /etc/live-iso-volid

# --- accounts / ssh ------------------------------------------------------------
# No default password anywhere: root is LOCKED on the live image (consoles autologin, see
# live-installer service; sshd is off). The installer sets the real password / SSH key.
passwd -l root >/dev/null
mkdir -p /etc/ssh/sshd_config.d
# a drop-in, never an edit of the openssh-owned sshd_config (edits became a ._cfg0000_sshd_config conflict on every openssh
# update of installed systems); the installer overwrites this same file with the install choice (liveinst SSHD_DROPIN)
printf '# live image default (the installer rewrites this file)\nPermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitEmptyPasswords no\n' > /etc/ssh/sshd_config.d/00-horcrux-installer.conf
grep -q '^Include "\?/etc/ssh/sshd_config.d/\*.conf' /etc/ssh/sshd_config || { echo "stock sshd_config lost its sshd_config.d Include" >&2; exit 1; }
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
# systemd images use the SAME script-based dracut initramfs as the OpenRC images (dmsquash-live, zfs, the horcrux-unlock/tpm-unlock modules,
# their hooks and prompts are then identical on both inits; systemd starts as PID 1 after switch_root). docs/design/init-parity.md, phase 3.
# Own file: it stays on the installed system, so every kernel update builds the same kind of initramfs.
if [ "$INIT" = systemd ]; then
  # every systemd dracut module (systemd, systemd-*, dracut-systemd: e.g. systemd-battery-check errors out when only `systemd` is omitted), read
  # from the installed modules so a dracut update that adds one cannot break the build. Modules that only use systemd WHEN it is included
  # (resume, squash-lib, tpm2-tss, ...) stay.
  sdmods=$(for d in /usr/lib/dracut/modules.d/[0-9]*; do n=${d##*/}; n=${n#[0-9][0-9]}; \
           case "$n" in systemd|systemd-*|dracut-systemd) echo "$n" ;; esac; done | sort -u | tr '\n' ' ')
  echo "dracut modules omitted (systemd initrd): $sdmods"
  printf '# Horcrux/Patronus systemd images: script-based initramfs (same as the OpenRC images); systemd runs from the real root.\n# setup-rootfs.sh lists every module that is or needs systemd.\nomit_dracutmodules+=" %s "\n' "$sdmods" > /etc/dracut.conf.d/05-init.conf
fi
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
