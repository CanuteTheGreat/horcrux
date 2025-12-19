# Catalyst spec for Horcrux Live CD (stage 1)

subarch: amd64
target: livecd-stage1
version_stamp: horcrux-latest
rel_type: default
profile: default/linux/amd64/23.0
source_subpath: default/stage3-amd64-openrc-latest

# Portage snapshot (HEAD of gentoo.git)
snapshot_treeish: HEAD
portage_confdir: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/portage

# LiveCD stage1 USE flags
livecd/use:
    -systemd
    openrc
    livecd
    ncurses
    readline
    ssl

# LiveCD stage1 packages
livecd/packages:
    # Base system
    app-admin/sudo
    app-admin/sysklogd
    app-misc/screen
    app-misc/tmux
    app-editors/vim
    app-editors/nano

    # Networking
    net-misc/curl
    net-misc/wget
    net-misc/openssh
    net-misc/dhcpcd

    # Utilities
    dev-vcs/git
    sys-apps/busybox
    sys-apps/pciutils
    sys-apps/usbutils
    sys-process/htop

    # Disk/filesystem tools
    sys-block/parted
    sys-fs/dosfstools
    sys-fs/e2fsprogs
    sys-fs/xfsprogs
    sys-fs/btrfs-progs
    sys-fs/lvm2
    sys-fs/squashfs-tools

    # Boot
    sys-boot/grub

    # Kernel
    sys-kernel/gentoo-kernel-bin
    sys-kernel/linux-firmware
