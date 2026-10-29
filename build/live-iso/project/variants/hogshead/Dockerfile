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
    for e in /var/db/repos/horcrux/sys-kernel/horcrux-t2-config/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-bin/*.ebuild; do ebuild "$e" manifest; done

# the T2 kernel: patches+config package first, then the dist kernel built FROM SOURCE with them; the stock binary kernel goes
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --oneshot --usepkg=n sys-kernel/horcrux-t2-config; \
    emerge --unmerge sys-kernel/gentoo-kernel-bin virtual/dist-kernel || true; \
    emerge --oneshot --usepkg=n --getbinpkg=n =sys-kernel/gentoo-kernel-7.2.13 \
        > /var/log/hogshead-kernel-build.log 2>&1 || { tail -80 /var/log/hogshead-kernel-build.log; exit 1; }; \
    ls /lib/modules /boot

# ZFS built for THAT kernel (never from a cached binpkg of another release), dracut module package, kernel sanity
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --oneshot --usepkg=n --getbinpkg=n =sys-fs/zfs-kmod-2.3.9 =sys-fs/zfs-2.3.9; \
    emerge --oneshot sys-kernel/horcrux-unlock; \
    kr=$(ls /lib/modules | grep -- '-t2$' | tail -1); test -n "$kr"; \
    find /lib/modules/$kr -name 'zfs.ko*' | grep .; \
    for m in t2bce_core t2bce_vhci t2bce_dma hid-apple hid-appletb-kbd; do find /lib/modules/$kr -name "$m.ko*" | grep . ; done; \
    for r in $(grep -v '^#' /usr/share/horcrux-t2/required-config.txt | grep .); do grep -qx "$r" /usr/src/linux-$(echo $kr | sed 's/-gentoo-dist-t2//')-gentoo-dist/.config || echo "WARN missing $r"; done; \
    depmod $kr; echo $kr > /etc/hogshead-kernelrelease

# the owner's applications (Portage packages: binhost binpkgs where USE matches, the rest compiles)
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --update --newuse --usepkg --getbinpkg --keep-going \
        app-containers/docker app-containers/docker-compose app-containers/docker-buildx \
        net-misc/remmina media-gfx/gimp media-gfx/geeqie net-misc/mosh net-analyzer/nmap \
        net-im/discord net-vpn/proton-vpn-gtk-app app-admin/rustinion-bin \
        sys-firmware/edk2-bin app-admin/sudo sys-process/cronie sys-apps/portage-utils app-portage/eix \
        > /var/log/hogshead-apps.log 2>&1 || { tail -60 /var/log/hogshead-apps.log; exit 1; }

# optional heavy extras: failures are logged, never silent
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; export FEATURES="-ccache buildpkg"; \
    for p in app-emulation/libguestfs app-emulation/libguestfs-appliance; do \
      emerge --update --newuse --usepkg --getbinpkg $p > /var/log/hogshead-opt-$(basename $p).log 2>&1 \
        && echo "OPTIONAL-OK $p" || echo "OPTIONAL-FAILED $p"; \
    done | tee /etc/hogshead-optional.txt; true

RUN set -e; for s in docker; do rc-update add $s default || true; done; \
    echo "hogshead" >> /etc/live-iso-variant; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    rm -rf /var/tmp/portage /var/cache/distfiles/* 2>/dev/null || true

# binary packages of what this variant built (kernel, zfs, horcrux-unlock, rustinion, config): a binhost-ready PKGDIR
FROM hogshead AS binpkgs-build
RUN --mount=type=cache,id=binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    mkdir -p /export/sys-kernel /export/sys-fs /export/app-admin; \
    cp -a /var/cache/binpkgs/sys-kernel/horcrux-* /var/cache/binpkgs/sys-kernel/gentoo-kernel-7.2.13* /export/sys-kernel/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/sys-fs/zfs-2.3.9* /var/cache/binpkgs/sys-fs/zfs-kmod-2.3.9* /export/sys-fs/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/app-admin/rustinion-bin* /export/app-admin/ 2>/dev/null; \
    cp -a /var/cache/binpkgs/Packages /export/ 2>/dev/null; find /export -type f | head -50

FROM scratch AS binpkgs-export
COPY --from=binpkgs-build /export /
