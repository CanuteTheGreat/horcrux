#!/bin/sh
# guest-checks.sh: run inside the booted live ISO (over ssh by qemu-test.py). One line per check: "PASS|FAIL name :: detail".
# env: FW_SHA (sha256 of brcmfmac4364b3-pcie.apple,kure.bin, optional), EXPECT_KEYS (number of baked authorized_keys, optional)
kr=$(uname -r)
r() { if [ "$1" = 0 ]; then echo "PASS $2 :: $3"; else echo "FAIL $2 :: $3"; fi; }

echo "$kr" | grep -q 't2-horcrux'; r $? kernel-release "$kr"
for o in pm_async=off intel_iommu=on iommu=pt; do grep -q "$o" /proc/cmdline; r $? "cmdline-$o" "$(cat /proc/cmdline | cut -c1-200)"; done

# ---- kernel config has the T2 options
cfg=/boot/config-$kr
for o in CONFIG_T2BCE_CORE=m CONFIG_T2BCE_DMA=m CONFIG_T2BCE_VHCI=m CONFIG_T2BCE_AUDIO=m CONFIG_HID_APPLE=m CONFIG_HID_APPLETB_KBD=m \
         CONFIG_HID_APPLETB_BL=m CONFIG_BRCMFMAC=m CONFIG_BRCMFMAC_PCIE=y CONFIG_BT_HCIBCM4377=m CONFIG_DRM_I915=m CONFIG_BLK_DEV_NVME=m \
         CONFIG_USB4=m CONFIG_TYPEC=m CONFIG_DM_CRYPT=m CONFIG_SENSORS_APPLESMC=m CONFIG_DRM_APPLETBDRM=m; do
  grep -qx "$o" "$cfg"; r $? "kconfig-$o" "$cfg"
done

# ---- modules built for this kernel (names normalised: - and _ are the same)
mods=$(find /lib/modules/$kr -name '*.ko*' | sed 's#.*/##; s#\.ko.*##' | tr - _)
for m in t2bce_core t2bce_dma t2bce_vhci t2bce_audio hid_apple hid_appletb_kbd hid_appletb_bl brcmfmac brcmfmac_wcc i915 nvme thunderbolt zfs apple_mfi_fastcharge appletbdrm dm_crypt; do
  echo "$mods" | grep -qx "$m"; r $? "module-$m" present
done

# ---- initramfs (live) carries the early input modules
img=$(ls /boot/initramfs-*.img | head -1)
list=$(lsinitrd "$img" 2>/dev/null | tr - _)
for m in t2bce_core t2bce_dma t2bce_vhci hid_apple hid_appletb_kbd hid_appletb_bl usbhid i915 nvme dm_crypt; do
  echo "$list" | grep -q "/$m\.ko"; r $? "initramfs-$m" "$img"
done
lsinitrd "$img" 2>/dev/null | tr - _ | grep -rq 'force_driver\|rd.driver.pre' ; echo "INFO initramfs-force-drivers :: $(lsinitrd -f etc/cmdline.d/20-force_driver.conf "$img" 2>/dev/null | head -2 | tr '\n' ' ')"
# the ISO's own initramfs is what booted: boot medium copy
echo "INFO booted-initramfs-size :: $(du -h "$img" | cut -f1)"

# ---- firmware (private ISO only)
n=$(ls /lib/firmware/brcm/ | grep -c 'brcmfmac4364b3-pcie.apple')
[ "$n" -ge 10 ]; r $? firmware-brcmfmac4364b3-apple "$n files"
nb=$(ls /lib/firmware/brcm/ | grep -c '^brcmbt.*apple')
[ "$nb" -ge 4 ]; r $? firmware-bluetooth-apple "$nb files"
if [ -n "${FW_SHA:-}" ]; then
  got=$(sha256sum '/lib/firmware/brcm/brcmfmac4364b3-pcie.apple,kure.bin' | cut -d' ' -f1)
  [ "$got" = "$FW_SHA" ]; r $? firmware-sha256-kure.bin "$got"
fi

if [ -n "${TS_SHA:-}" ]; then
  got=$(sha256sum /etc/hogshead/ts-authkey 2>/dev/null | cut -d' ' -f1); m=$(stat -c %a /etc/hogshead/ts-authkey 2>/dev/null)
  [ "$got" = "$TS_SHA" ] && [ "$m" = 600 ]; r $? baked-tailscale-authkey "present, mode $m, sha256 matches the key created for this build (value not printed)"
fi
# ---- ssh: key-only
/usr/sbin/sshd -T 2>/dev/null | grep -qix 'passwordauthentication no'; r $? sshd-passwordauth-off ""
/usr/sbin/sshd -T 2>/dev/null | grep -Eqix 'permitrootlogin (prohibit-password|without-password)'; r $? sshd-root-key-only ""
if [ -n "${EXPECT_KEYS:-}" ]; then
  c=$(grep -c '^\(ssh\|ecdsa\)' /root/.ssh/authorized_keys)
  [ "$c" -ge "$EXPECT_KEYS" ]; r $? authorized_keys "$c keys (>= $EXPECT_KEYS expected)"
fi
rc-service sshd status >/dev/null 2>&1; r $? sshd-running ""
[ "$(hostname)" = hogshead-live ]; r $? hostname "$(hostname)"

# ---- userland
for c in nmtui nmcli cryptsetup lvm mkfs.ext4 resize2fs sgdisk parted efibootmgr mkfs.vfat smartctl nvme vim tmux curl rsync lspci lsusb tailscale tailscaled zpool zfs rsync sshd hogshead-remote hogshead-unlock hogshead-backup hogshead-motd; do
  command -v $c >/dev/null; r $? "tool-$c" "$(command -v $c)"
done
modprobe zfs 2>/dev/null; zpool version 2>&1 | head -2 | sed 's/^/INFO zfs :: /'
[ -f /sys/module/zfs/version ]; r $? zfs-module-loaded "$(cat /sys/module/zfs/version 2>/dev/null)"
modinfo -F vermagic zfs 2>/dev/null | grep -q "$kr"; r $? zfs-vermagic-matches-kernel "$(modinfo -F vermagic zfs 2>/dev/null)"
rc-service NetworkManager status >/dev/null 2>&1; r $? networkmanager-running ""
tailscale version | head -1 | sed 's/^/INFO tailscale :: /'
[ ! -e /etc/runlevels/default/tailscaled ]; r $? tailscaled-not-autostarted ""
rc-status -a 2>/dev/null | sed 's/^/INFO rc :: /' | head -30
