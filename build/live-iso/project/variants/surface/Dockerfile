# syntax=docker/dockerfile:1.4
# Variant `surface` (2nd half of the Surface image; applied on top of `surface-kernel`): the installer package, the XFCE tablet desktop
# and the owner's apps as Portage packages, cleanup and the live initramfs.
#   Build context = build/live-iso, named context `hxgentoo` = the repo's gentoo/ (the horcrux overlay).
ARG BASE
FROM ${BASE} AS surface-base
ARG ARCH=x86_64
# the full overlay + the Portage config owned by app-misc/horcrux-surface-desktop (identical bytes: installing the package later is a no-op
# for config protection)
COPY --from=hxgentoo / /var/db/repos/horcrux/
RUN cp -a /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/files/root/etc/portage/. /etc/portage/ \
 && for e in /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-bin/*.ebuild; do ebuild "$e" manifest >/dev/null; done

# the installer / live scripts / console menu that the ISO's own tree provides become ONE package (sys-apps/horcrux-live-installer), made
# from common/overlay + project/overlay at build time; Horcrux's hypervisor product and the Apple T2 userland are not tablet roles: removed
FROM surface-base AS surface-live
COPY common/overlay/ /tmp/hxlive/common/
COPY project/overlay/ /tmp/hxlive/project/
COPY common/portage/package.use/ /tmp/hxlive/portage-use/
COPY common/portage-gui/package.use /tmp/hxlive/portage-use/gui
RUN --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; cd /tmp/hxlive; \
    rm -rf project/usr/share/horcrux/t2 project/etc/init.d/t2fanrd project/etc/init.d/tiny-dfr project/etc/init.d/horcrux-api project/etc/conf.d/horcrux-api; \
    mkdir -p /tmp/pkg; cp -a common/. /tmp/pkg/; cp -a project/. /tmp/pkg/; mkdir -p /tmp/pkg/etc/portage/package.use; cp -a portage-use/. /tmp/pkg/etc/portage/package.use/; \
    tar -C /tmp/pkg --zstd -cf /var/cache/distfiles/horcrux-live-installer-1.tar.zst .; \
    ebuild /var/db/repos/horcrux/sys-apps/horcrux-live-installer/horcrux-live-installer-1.ebuild manifest; \
    CONFIG_PROTECT="-*" emerge --oneshot --usepkg=n --getbinpkg=n sys-apps/horcrux-live-installer; \
    rm -rf /tmp/hxlive /tmp/pkg; \
    rm -f /usr/bin/horcrux /usr/bin/horcrux-api /usr/bin/tiny-dfr /usr/bin/t2fanrd /etc/init.d/horcrux-guests /etc/init.d/horcrux-api /etc/init.d/t2fanrd /etc/init.d/tiny-dfr \
          /usr/lib/udev/rules.d/99-touchbar-tiny-dfr.rules /usr/lib/udev/rules.d/99-touchbar-seat.rules; \
    rm -rf /usr/share/horcrux /usr/share/tiny-dfr /var/lib/horcrux /var/log/horcrux /etc/horcrux; \
    rc-update del horcrux-api default 2>/dev/null || true; \
    qfile -o /usr/bin/live-installer /etc/init.d/live-installer; qlist -Iv sys-apps/horcrux-live-installer

# the tablet desktop + the owner's apps = ONE meta package (binpkgs where USE matches; the rest compiles here, never on the tablet)
FROM surface-live AS surface-apps
# the Nextcloud client first and on its own: it is the one big Qt source build, and a failure must show its own build log.
# nextcloud-client 4.0.4 configures with find_package(ECM) even without USE=dolphin (Gentoo only lists ECM for dolphin): the meta package carries ECM
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    eselect profile set default/linux/amd64/23.0/desktop; \
    emerge --update --newuse --usepkg --getbinpkg kde-frameworks/extra-cmake-modules net-misc/nextcloud-client > /var/log/surface-nc.log 2>&1 || { tail -30 /var/log/surface-nc.log; for l in /var/tmp/portage/*/*/temp/build.log; do echo "== $l"; grep -n -i -E "error|undefined|fatal|CMake Error" $l | head -30; tail -25 $l; done; exit 1; }
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --update --newuse --usepkg --getbinpkg app-misc/horcrux-surface-desktop \
        > /var/log/surface-apps.log 2>&1 || { tail -80 /var/log/surface-apps.log; for l in /var/tmp/portage/*/*/temp/build.log; do echo "== $l"; tail -25 $l; done; exit 1; }; \
    tail -5 /var/log/surface-apps.log

FROM surface-apps AS surface-final
# Horcrux's own product (hypervisor/containers) is not a tablet role: drop it from world and the filesystem, so nothing unowned remains
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --deselect app-emulation/qemu app-emulation/libvirt app-containers/docker app-containers/docker-cli net-misc/bridge-utils net-dns/dnsmasq 2>&1 | tail -3; \
    emerge --depclean --with-bdeps=n 2>&1 | tail -15 || true

# the live initramfs for the new kernel (dmsquash-live + the zfs/lvm/resume modules the installer's initramfs needs), services for the live image
RUN set -e; rm -rf /lib/modules/*-gentoo-dist-bin /usr/src/linux-6.18.57* /etc/libvirt /var/lib/libvirt; ls /lib/modules
RUN set -e; kr=$(cat /etc/surface-kernelrelease); \
    export CHOST=$(ls /usr/lib/gcc | head -1); \
    dracut -f --no-hostonly --kver "$kr" "/boot/initramfs-$kr.img" "$kr"; \
    ls -l /boot; \
    echo "surface" >> /etc/live-iso-variant; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    rm -rf /var/tmp/portage /var/cache/distfiles/* /usr/share/doc /usr/share/gtk-doc 2>/dev/null || true
