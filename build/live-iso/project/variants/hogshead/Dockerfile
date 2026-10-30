# syntax=docker/dockerfile:1.4
# Horcrux hogshead variant (Apple T2 laptop, EVERYTHING owned by a Portage package):
#   BASE (desktop rootfs) + overlays (horcrux, GURU, machine-local) + T2 kernel as sys-kernel/gentoo-kernel
#   (source-built, t2linux patches/config from sys-kernel/horcrux-t2-config) + zfs-kmod rebuilt for it +
#   sys-kernel/horcrux-unlock (dracut) + the owner's applications. Build context = build/live-iso,
#   named context `hxgentoo` = the repo's gentoo/ (the horcrux overlay).
ARG BASE
FROM ${BASE} AS hogshead
ARG ARCH=x86_64

COPY --from=hxgentoo / /var/db/repos/horcrux/
COPY project/variants/hogshead/local-repo/ /var/db/repos/local/
COPY project/variants/hogshead/portage/ /etc/portage/

# tree + GURU overlay (git sync) + manifests of the local ebuilds
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    emerge-webrsync -q || emerge --sync -q; \
    export FEATURES="-ccache"; \
    emerge --noreplace --usepkg --getbinpkg app-eselect/eselect-repository dev-vcs/git; \
    eselect repository enable guru; \
    emerge --sync guru; \
    for e in /var/db/repos/horcrux/sys-kernel/horcrux-t2-config/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-bin/*.ebuild /var/db/repos/horcrux/sys-apps/t2fanrd/*.ebuild /var/db/repos/horcrux/sys-apps/tiny-dfr/*.ebuild; do ebuild "$e" manifest; done

# T2 patches + config package (stage `hs-prep` is shared by the checks below)
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg=n sys-kernel/horcrux-t2-config; \
    ls /etc/portage/patches/sys-kernel/gentoo-kernel | wc -l; ls /etc/kernel/config.d

# quick check (minutes, not an hour): the patches apply and the config fragment wins (kernel release must end in -t2)
FROM hogshead AS hs-kconfig
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    ebuild /var/db/repos/gentoo/sys-kernel/gentoo-kernel/gentoo-kernel-7.2.13.ebuild clean configure 2>&1 | tail -30; \
    w=/var/tmp/portage/sys-kernel/gentoo-kernel-7.2.13/work/linux-7.2; \
    grep -E 'LOCALVERSION|CONFIG_T2BCE|CONFIG_HID_APPLETB_KBD|CONFIG_APPLE_GMUX' $w/.config; \
    ls $w/drivers/staging/t2bce; ls /var/tmp/portage/sys-kernel/gentoo-kernel-7.2.13/temp/ | head

# the kernel binary package is built in its OWN layer (about an hour) and kept inside the image layer:
# a later failure (install, dracut) does not recompile it
FROM hogshead AS hs-kbuild
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/hs-binpkgs; \
    emerge --buildpkgonly --nodeps --usepkg=n --getbinpkg=n =sys-kernel/gentoo-kernel-7.2.13 \
        > /var/log/hogshead-kernel-build.log 2>&1 || { tail -80 /var/log/hogshead-kernel-build.log; exit 1; }; \
    find /var/cache/hs-binpkgs -type f | head

FROM hs-kbuild AS hs-kinstall
# the stock binary kernel goes; the freshly built kernel binpkg is installed. dracut cannot add the zfs module before zfs-kmod
# exists for this kernel: omit zfs/unlock for THIS step only (the installer builds the real initramfs at install time)
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/hs-binpkgs; \
    echo 'omit_dracutmodules+=" zfs horcrux-unlock "' > /etc/dracut.conf.d/00-hogshead-build.conf; \
    emerge --unmerge sys-kernel/gentoo-kernel-bin || true; \
    emerge --oneshot --usepkgonly --nodeps =sys-kernel/gentoo-kernel-7.2.13; \
    ls /lib/modules /boot

# ZFS built for THAT kernel (never from a cached binpkg of another release), dracut module package, kernel sanity
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/hs-binpkgs; \
    emerge --oneshot --usepkg=n --getbinpkg=n =sys-fs/zfs-kmod-2.3.9 =sys-fs/zfs-2.3.9; \
    rm -f /etc/dracut.conf.d/00-hogshead-build.conf; \
    emerge --oneshot sys-kernel/horcrux-unlock; \
    kr=$(ls /lib/modules | grep -- '-t2$' | tail -1); test -n "$kr"; \
    find /lib/modules/$kr -name 'zfs.ko*' | grep .; \
    for m in t2bce_core t2bce_vhci t2bce_dma hid-apple hid-appletb-kbd; do find /lib/modules/$kr -name "$m.ko*" | grep . ; done; \
    depmod $kr; echo $kr > /etc/hogshead-kernelrelease

FROM hs-kinstall AS hogshead-final
# the owner's applications (Portage packages: binhost binpkgs where USE matches, the rest compiles)
RUN --mount=type=cache,id=binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --update --newuse --usepkg --getbinpkg --keep-going \
        app-containers/docker app-containers/docker-compose app-containers/docker-buildx \
        net-misc/remmina media-gfx/gimp media-gfx/geeqie net-misc/mosh net-analyzer/nmap \
        net-im/discord net-vpn/proton-vpn-gtk-app app-admin/rustinion-bin sys-apps/t2fanrd sys-apps/tiny-dfr \
        sys-firmware/edk2-bin app-admin/sudo sys-process/cronie sys-apps/portage-utils app-portage/eix \
        > /var/log/hogshead-apps.log 2>&1 || { tail -60 /var/log/hogshead-apps.log; exit 1; }

# optional heavy extras: failures are logged, never silent
RUN --mount=type=cache,id=binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; export FEATURES="-ccache buildpkg"; \
    for p in app-emulation/libguestfs app-emulation/libguestfs-appliance; do \
      emerge --update --newuse --usepkg --getbinpkg $p > /var/log/hogshead-opt-$(basename $p).log 2>&1 \
        && echo "OPTIONAL-OK $p" || echo "OPTIONAL-FAILED $p"; \
    done | tee /etc/hogshead-optional.txt; true

RUN set -e; for s in docker t2fanrd tiny-dfr; do rc-update add $s default || true; done; \
    echo "hogshead" >> /etc/live-iso-variant; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    rm -rf /var/tmp/portage /var/cache/distfiles/* 2>/dev/null || true

# binary packages of what this variant built (kernel, zfs, horcrux-unlock, rustinion, config): a binhost-ready PKGDIR
FROM hogshead-final AS binpkgs-build
RUN --mount=type=cache,id=binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    mkdir -p /export/sys-kernel /export/sys-fs /export/app-admin; \
    cp -a /var/cache/binpkgs/sys-kernel/horcrux-* /var/cache/binpkgs/sys-kernel/gentoo-kernel-7.2.13* /export/sys-kernel/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/sys-fs/zfs-2.3.9* /var/cache/binpkgs/sys-fs/zfs-kmod-2.3.9* /export/sys-fs/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/app-admin/rustinion-bin* /export/app-admin/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/Packages /export/ 2>/dev/null; find /export -type f | head -50

FROM scratch AS binpkgs-export
COPY --from=binpkgs-build /export /
