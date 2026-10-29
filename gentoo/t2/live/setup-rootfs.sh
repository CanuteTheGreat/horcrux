#!/bin/sh
# final configuration of the hogshead live rootfs (runs inside docker build). /tmp/priv = PRIVATE inputs (optional).
set -eu
chmod +x /usr/bin/hogshead-* /etc/init.d/tailscaled /etc/init.d/hogshead-live
rm -f /.dockerenv
sed -i 's/^#\?rc_sys=.*/rc_sys=""/' /etc/rc.conf
add() { rc-update add "$1" "$2" >/dev/null 2>&1 || { echo "FATAL: rc-update add $1 $2 (no such service?)"; ls /etc/init.d | tr '\n' ' '; exit 1; }; }
addopt() { rc-update add "$1" "$2" >/dev/null 2>&1 || echo "warn: rc-update add $1 $2 failed (optional)"; }
for s in devfs dmesg udev; do add $s sysinit; done
for s in modules hostname bootmisc sysctl procfs termencoding root localmount swclock consolefont; do add $s boot; done
for s in seedrng urandom; do addopt $s boot; done
for s in sysklogd dbus NetworkManager hogshead-live sshd local; do add $s default; done
# NOTE: tailscaled is NOT enabled: `hogshead-remote` starts it
printf '# live system: overlay root\ntmpfs /tmp tmpfs defaults,nosuid,nodev 0 0\n' > /etc/fstab
echo hogshead-live > /etc/hostname
# swclock restores the last saved time at boot: give it the build date instead of the stage3's
mkdir -p /lib/rc/cache && touch /lib/rc/cache/shutdowntime
cat > /etc/os-release <<OSR
NAME="Hogshead live (Horcrux/Gentoo, T2 kernel)"
ID=hogshead-live
ID_LIKE=gentoo
PRETTY_NAME="Hogshead live USB (Gentoo + t2linux kernel)"
OSR
echo HOGSHEAD_LIVE > /etc/live-iso-volid
# --- console: tty1..tty3 login prompts, tty1 autologin as root (root has no password; ssh is key-only)
passwd -l root >/dev/null
sed -i -E '/^#?[a-z][0-9a-z]*:[0-9]*:respawn:\/sbin\/agetty .*tty(S|AMA|AML|MXC|USB)[0-9]+ /d' /etc/inittab
sed -i -E 's#^c1:.*#c1:12345:respawn:/sbin/agetty --autologin root --noclear 38400 tty1 linux#' /etc/inittab
grep -q 'autologin root' /etc/inittab || { echo "FATAL: inittab c1 not rewritten"; cat /etc/inittab; exit 1; }
for t in tty1 tty2 tty3 ttyS0 console; do grep -qx $t /etc/securetty 2>/dev/null || echo $t >> /etc/securetty; done
# --- ssh keys (PUBLIC keys of the owner) + sshd config (first match wins: neutralise stock defaults)
mkdir -p /root/.ssh; chmod 700 /root/.ssh
[ -f /tmp/priv/authorized_keys ] && install -m 0600 /tmp/priv/authorized_keys /root/.ssh/authorized_keys
grep -q 'sshd_config.d' /etc/ssh/sshd_config || sed -i '1i Include /etc/ssh/sshd_config.d/*.conf' /etc/ssh/sshd_config
sed -i -E 's/^#?(PermitRootLogin|PasswordAuthentication).*//' /etc/ssh/sshd_config
printf 'PermitRootLogin prohibit-password\nPasswordAuthentication no\n' >> /etc/ssh/sshd_config
# --- private: Apple firmware + tailscale one-time key
mkdir -p /etc/hogshead /lib/firmware/brcm
if [ -d /tmp/priv/fw ]; then cp -a /tmp/priv/fw/. /lib/firmware/brcm/; fi
if [ -s /tmp/priv/ts-authkey ]; then install -m 0600 /tmp/priv/ts-authkey /etc/hogshead/ts-authkey; fi
# --- prune firmware this laptop/tests never use (ISO must stay well under the 2 GB Forgejo asset limit)
cd /lib/firmware
for d in amdgpu radeon nvidia netronome mellanox qcom ti-connectivity ath10k ath11k ath12k mrvl mediatek liquidio cxgb4 bnx2x qed qlogic rockchip imx amlogic cirrus; do rm -rf "$d"; done
cd /
# --- initramfs (T2 modules early: dracut.conf.d/90-t2.conf)
kr=$(cat /boot/.kernelrelease)
echo "kernel: $kr"
dracut -f --no-hostonly --kver "$kr" "/boot/initramfs-$kr.img" "$kr"
ls -l /boot
for m in t2bce_core t2bce_vhci hid_apple hid_appletb_kbd hid_appletb_bl; do
  lsinitrd "/boot/initramfs-$kr.img" | tr - _ | grep -q "/$m\.ko" || { echo "FATAL: $m missing from the initramfs"; exit 1; }
done
# --- slim
rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage \
       /var/log/emerge* /usr/src/linux* /usr/share/man /usr/share/info 2>/dev/null || true
find /lib/modules/$kr -name '*.ko' -size +0 >/dev/null
echo "/lib/modules size: $(du -sh /lib/modules | cut -f1)  firmware: $(du -sh /lib/firmware | cut -f1)"
