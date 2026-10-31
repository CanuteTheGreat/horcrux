# syntax=docker/dockerfile:1.4
# Variant `surface` (2nd half of the Surface image; applied on top of `surface-kernel`): the installer package, the XFCE tablet desktop
# and the owner's apps as Portage packages, cleanup and the live initramfs.
#   Build context = build/live-iso, named context `hxgentoo` = the repo's gentoo/ (the horcrux overlay).
ARG BASE
FROM ${BASE} AS surface-base
ARG ARCH=x86_64
# the full overlay + the Portage config owned by app-misc/horcrux-surface-desktop (identical bytes: installing the package later is a no-op
# for config protection)
RUN rm -rf /var/db/repos/horcrux
COPY --from=hxgentoo / /var/db/repos/horcrux/
RUN cp -a /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/files/root/etc/portage/. /etc/portage/ \
 && for e in /var/db/repos/horcrux/app-misc/horcrux-surface-desktop/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-bin/*.ebuild; do ebuild "$e" manifest >/dev/null; done

# the installer / live scripts / console menu come from the repository tree as the two PROVEN generic packages (same recipe as the hogshead
# variant, Dockerfile.part2): sys-apps/horcrux-tools (installer, scripts, horcrux-orphans, + the live-ISO /etc payload for this image) and
# sys-apps/horcrux-live-data (branding + portage-meta snapshot of THIS build). sys-kernel/horcrux-unlock owns the dracut module.
# Horcrux's hypervisor product and the Apple T2 userland are not tablet roles: left out of the tarball.
FROM surface-base AS surface-live
COPY common/overlay/ /tmp/hxlive/common/
COPY project/overlay/ /tmp/hxlive/project/
COPY common/portage/package.use/ /tmp/hxlive/portage-use/
COPY common/portage-gui/package.use /tmp/hxlive/portage-use/gui
COPY project/variants/hogshead/overlay/usr/bin/horcrux-orphans /tmp/hxlive/horcrux-orphans
RUN --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; cd /tmp/hxlive; \
    rm -rf project/usr/share/horcrux project/etc/init.d/t2fanrd project/etc/init.d/tiny-dfr project/etc/init.d/horcrux-api project/etc/conf.d/horcrux-api \
           common/usr/lib/dracut; \
    mkdir -p /tmp/pkg; cp -a common/. /tmp/pkg/; cp -a project/. /tmp/pkg/; rm -rf /tmp/pkg/usr/share; \
    mkdir -p /tmp/pkg/etc/portage/package.use; cp -a portage-use/. /tmp/pkg/etc/portage/package.use/; \
    install -m 0755 horcrux-orphans /tmp/pkg/usr/bin/horcrux-orphans; \
    tar -C /tmp/pkg --zstd -cf /var/cache/distfiles/horcrux-tools-1.tar.zst .; \
    tar -C / --zstd --exclude=usr/share/live-installer/hwprofiles -cf /var/cache/distfiles/horcrux-live-data-1.tar.zst usr/share/live-installer; \
    for e in /var/db/repos/horcrux/sys-apps/horcrux-tools/*.ebuild /var/db/repos/horcrux/sys-apps/horcrux-live-data/*.ebuild; do ebuild "$e" manifest; done; \
    rm -rf /usr/lib/live-installer /usr/libexec/live-* /usr/libexec/console-menu-session /usr/bin/live-installer /usr/bin/console-menu \
           /usr/bin/live-persist /usr/bin/live-rescue /usr/bin/horcrux-be /usr/bin/horcrux-t2-firmware /usr/share/live-installer \
           /usr/lib/dracut/modules.d/91horcrux-unlock /etc/profile.d/console-menu.sh /etc/init.d/live-* /etc/init.d/zram-swap /etc/conf.d/zram-swap \
           /etc/live-installer/profile.toml /etc/dracut.conf.d/10-microcode.conf /etc/motd; \
    CONFIG_PROTECT="-*" emerge --oneshot --usepkg=n --getbinpkg=n sys-kernel/horcrux-unlock sys-apps/horcrux-tools sys-apps/horcrux-live-data; \
    emerge --noreplace --select=y sys-kernel/horcrux-unlock sys-apps/horcrux-tools sys-apps/horcrux-live-data; \
    rm -rf /tmp/hxlive /tmp/pkg; \
    rm -f /usr/bin/horcrux /usr/bin/horcrux-api /usr/bin/tiny-dfr /usr/bin/t2fanrd /etc/init.d/horcrux-guests /etc/init.d/horcrux-api /etc/init.d/t2fanrd /etc/init.d/tiny-dfr \
          /usr/lib/udev/rules.d/99-touchbar-tiny-dfr.rules /usr/lib/udev/rules.d/99-touchbar-seat.rules; \
    rm -rf /usr/share/horcrux /usr/share/tiny-dfr /var/lib/horcrux /var/log/horcrux /etc/horcrux; \
    rc-update del horcrux-api default 2>/dev/null || true; \
    qfile -o /usr/bin/live-installer /etc/init.d/live-installer; qlist -Iv sys-apps/horcrux-tools sys-apps/horcrux-live-data sys-kernel/horcrux-unlock

# the tablet desktop + the owner's apps = ONE meta package (binpkgs where USE matches; the rest compiles here, never on the tablet)
FROM surface-live AS surface-apps
# dbus must have X (dbus-launch --autolaunch: the XFCE session bus). A plain `emerge -u <meta>` does not re-check already installed
# dependencies for USE changes, so rebuild it explicitly (the stock server image has dbus -X)
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --oneshot --newuse --usepkg --getbinpkg sys-apps/dbus > /var/log/surface-dbus.log 2>&1 || { tail -40 /var/log/surface-dbus.log; exit 1; }; \
    qlist -IUv sys-apps/dbus | grep -q ' X ' || { echo "dbus has no X: $(qlist -IUv sys-apps/dbus)"; exit 1; }
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    rm -f /var/cache/binpkgs/app-misc/horcrux-surface-desktop-* /var/cache/binpkgs/sys-apps/horcrux-tools-* /var/cache/binpkgs/sys-apps/horcrux-live-data-* /var/cache/binpkgs/sys-kernel/horcrux-surface-config-*; \
    emerge --update --newuse --usepkg --getbinpkg --usepkg-exclude "app-misc/horcrux-surface-desktop sys-apps/horcrux-tools sys-apps/horcrux-live-data sys-kernel/horcrux-unlock sys-kernel/horcrux-surface-config app-admin/rustinion-bin" app-misc/horcrux-surface-desktop \
        > /var/log/surface-apps.log 2>&1 || { tail -80 /var/log/surface-apps.log; for l in /var/tmp/portage/*/*/temp/build.log; do echo "== $l"; tail -25 $l; done; exit 1; }; \
    tail -5 /var/log/surface-apps.log

FROM surface-apps AS surface-final
# Horcrux's own product (hypervisor/containers) is not a tablet role: drop it from world and the filesystem, so nothing unowned remains
RUN --mount=type=cache,id=surface-binpkgs-${ARCH:-x86_64},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=surface-distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --deselect app-emulation/qemu app-emulation/libvirt app-containers/docker app-containers/docker-cli net-misc/bridge-utils net-dns/dnsmasq 2>&1 | tail -3; \
    emerge --depclean --pretend --with-bdeps=n > /var/log/surface-depclean-pretend.log 2>&1; tail -40 /var/log/surface-depclean-pretend.log; \
    for k in sys-apps/horcrux-tools sys-apps/horcrux-live-data sys-kernel/horcrux-unlock sys-kernel/horcrux-surface-config sys-kernel/gentoo-kernel app-misc/horcrux-surface-desktop sys-fs/zfs-kmod sys-kernel/dracut sys-boot/grub sys-fs/cryptsetup sys-fs/lvm2; do \
        grep -q "^ *$k-[0-9]" /var/log/surface-depclean-pretend.log && { echo "depclean would remove $k"; exit 1; }; done; \
    emerge --depclean --with-bdeps=n 2>&1 | tail -15; \
    for f in /etc/init.d/live-installer /usr/libexec/live-installer-session /usr/bin/live-installer /usr/bin/horcrux-surface-auto /usr/share/live-installer/hwprofiles/surface.toml /usr/sbin/lightdm /usr/bin/xfce4-session /usr/bin/onboard /usr/bin/brave/usr/bin/rustinion; do test -e $f || { echo "MISSING after depclean: $f"; exit 1; }; done

# the live initramfs for the new kernel (dmsquash-live + the zfs/lvm/resume modules the installer's initramfs needs), services for the live image
RUN set -e; rm -rf /lib/modules/*-gentoo-dist-bin /usr/src/linux-6.18.57* /boot/*6.18.57* /boot/*.old /etc/libvirt /var/lib/libvirt; ls /lib/modules
RUN set -e; kr=$(cat /etc/surface-kernelrelease); \
    export CHOST=$(ls /usr/lib/gcc | head -1); \
    dracut -f --no-hostonly --kver "$kr" "/boot/initramfs-$kr.img" "$kr"; \
    ls -l /boot; \
    echo "surface" >> /etc/live-iso-variant; \
    etc-update --automode -5 >/dev/null 2>&1 || true; find /etc -name '._cfg*' -print -delete; \
    horcrux-orphans | grep -E '[.]pyc$' | xargs -r rm -f; find / -xdev -name __pycache__ -type d -empty -delete 2>/dev/null || true; \
    rm -rf /var/tmp/portage /var/cache/distfiles/* /usr/share/doc /usr/share/gtk-doc 2>/dev/null || true
