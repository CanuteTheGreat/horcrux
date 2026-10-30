# syntax=docker/dockerfile:1.4
# Variant `surface-kernel` (1st half of the Surface image; VARIANT=surface-kernel,surface ISO_VNAME=surface-): the
# long-lived layers (about 2 h on a loaded host): overlays + kernel config package + the source-built gentoo-kernel binary package
# + zfs-kmod. Kept separate so that edits to the desktop/apps layers never recompile the kernel.
# Horcrux Surface variant (Surface Pro 1/2 tablet, 4 GB, everything owned by a Portage package):
#   BASE (the finished server rootfs) + overlays (horcrux, GURU, canutethegreat-overlay) + the Surface kernel as
#   sys-kernel/gentoo-kernel (source-built; linux-surface series + config from sys-kernel/horcrux-surface-config) +
#   zfs-kmod rebuilt for it + the XFCE tablet desktop and the owner's apps as ONE meta package (app-misc/horcrux-surface-desktop).
#   Build context = build/live-iso, named context `hxgentoo` = the repo's gentoo/ (the horcrux overlay).
ARG BASE
FROM ${BASE} AS surface
ARG ARCH=x86_64
ARG SURFACE_KERNEL=6.18.59

# overlays: the repos.conf + portage config are COPIED FROM the package that owns them (identical bytes: installing the package later
# is then a no-op for Portage's config protection)
COPY --from=hxgentoo / /var/db/repos/horcrux/
RUN mkdir -p /etc/portage/repos.conf && cp -a /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/files/root/etc/portage/. /etc/portage/

RUN --mount=type=cache,id=surface-binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge-webrsync -q || emerge --sync -q; \
    emerge --noreplace --usepkg --getbinpkg dev-vcs/git; \
    git clone -q --depth 1 https://github.com/gentoo-mirror/guru.git /var/db/repos/guru; \
    git clone -q --depth 1 https://git.canutethegreat.com/CanuteTheGreat/canutethegreat-overlay.git /var/db/repos/canutethegreat-overlay; \
    for e in /var/db/repos/horcrux/sys-kernel/horcrux-surface-config/*.ebuild /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-bin/*.ebuild; do ebuild "$e" manifest; done; \
    emerge --oneshot --usepkg=n sys-kernel/horcrux-surface-config; \
    ls /etc/portage/patches/sys-kernel/gentoo-kernel | wc -l; ls /etc/kernel/config.d

# the kernel binary package is built in its OWN layer (about an hour) and kept inside the image layer
FROM surface AS surface-kbuild
ARG SURFACE_KERNEL=6.18.59
RUN --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/surface-binpkgs; \
    emerge --buildpkgonly --nodeps --usepkg=n --getbinpkg=n =sys-kernel/gentoo-kernel-${SURFACE_KERNEL} \
        > /var/log/surface-kernel-build.log 2>&1 || { tail -80 /var/log/surface-kernel-build.log; exit 1; }; \
    find /var/cache/surface-binpkgs -type f | head

FROM surface-kbuild AS surface-kinstall
ARG SURFACE_KERNEL=6.18.59
# the stock binary kernel goes (modules, vmlinuz), the freshly built one is installed; dracut cannot add zfs/unlock before zfs-kmod
# exists for this kernel: omit them for THIS step only (the live initramfs is rebuilt below, the installer builds the installed one)
RUN --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/surface-binpkgs; \
    echo 'omit_dracutmodules+=" zfs "' > /etc/dracut.conf.d/00-surface-build.conf; \
    emerge --unmerge sys-kernel/gentoo-kernel-bin; rm -f /boot/initramfs-*.img /boot/System.map-* /boot/config-*; \
    emerge --oneshot --usepkgonly --nodeps =sys-kernel/gentoo-kernel-${SURFACE_KERNEL}; \
    ls /lib/modules /boot
RUN --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/surface-binpkgs; \
    emerge --oneshot --usepkg=n --getbinpkg=n sys-fs/zfs-kmod sys-fs/zfs; \
    rm -f /etc/dracut.conf.d/00-surface-build.conf; \
    kr=$(ls /lib/modules | sort -V | tail -1); test -n "$kr"; \
    find /lib/modules/$kr -name 'zfs.ko*' | grep .; \
    for m in hid-multitouch hid-microsoft wacom mwifiex mwifiex_pcie mwifiex_sdio btmrvl_sdio hid-sensor-accel-3d surface_aggregator; do find /lib/modules/$kr -name "$m.ko*" | grep . || { echo "MISSING $m"; exit 1; }; done; \
    c=/lib/modules/$kr/build/.config; test -r $c; \
    grep -v '^#' /usr/share/horcrux-surface/required-config.txt | while read -r l; do grep -qx "$l" $c || { echo "MISSING CONFIG $l"; exit 1; }; done; \
    echo $kr > /etc/surface-kernelrelease
