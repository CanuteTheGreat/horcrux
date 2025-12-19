# Catalyst spec for Horcrux Live CD (stage 2)

subarch: amd64
target: livecd-stage2
version_stamp: horcrux-latest
rel_type: default
profile: default/linux/amd64/23.0
source_subpath: default/livecd-stage1-amd64-horcrux-latest

# Portage snapshot (HEAD of gentoo.git)
snapshot_treeish: HEAD
portage_confdir: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/portage

# Boot configuration
livecd/bootargs: dokeymap
livecd/cdtar: /usr/share/catalyst/livecd/cdtar/isolinux-elilo-memtest86+-cdtar.tar.bz2
livecd/fstype: squashfs
livecd/iso: /var/tmp/catalyst/builds/horcrux-amd64-latest.iso

# Kernel
boot/kernel: gentoo
boot/kernel/gentoo/sources: gentoo-kernel-bin
boot/kernel/gentoo/use: symlink livecd

# LiveCD customization
livecd/type: gentoo-release-livecd
livecd/volid: HORCRUX
livecd/overlay: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/livecd_overlay
livecd/fsscript: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/fsscript-livecd.sh
livecd/root_overlay: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/root_overlay

# Users
livecd/users: horcrux
livecd/motd: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/motd.txt

# OpenRC services to enable
livecd/rcadd:
    sshd|default
    dhcpcd|default
    sysklogd|default
