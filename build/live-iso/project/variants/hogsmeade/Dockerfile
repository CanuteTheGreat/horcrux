# syntax=docker/dockerfile:1.4
# HOGSMEADE variant (Alienware 18 Area-51, Arrow Lake-HX + RTX 5090 Laptop): EVERYTHING owned by a Portage package.
#   BASE (server rootfs) -> desktop (same steps as variants/desktop, but the size strip is done LAST, here) ->
#   sys-kernel/gentoo-kernel built from source with sys-kernel/horcrux-hogsmeade-config -> zfs-kmod + nvidia-drivers
#   (kernel-open) rebuilt for THAT kernel -> firmware -> horcrux-unlock/tools -> hogsmeade-tools, awccd, alienfx, ComfyUI,
#   Rustinion, owner's apps -> private payload package.
# Build context = build/live-iso. Named contexts: hxgentoo = the repo's gentoo/ (horcrux overlay), ctgoverlay = canutethegreat-overlay,
# hsmprivate = directory with the private payload (agent_authorized_keys, backup.env, restic-password): NEVER in git.
ARG BASE
FROM ${BASE} AS hsm-prep
ARG ARCH=x86_64
COPY project/variants/gnome-light/portage/ /etc/portage/
COPY project/variants/hogsmeade/portage/ /etc/portage/
COPY --from=hxgentoo / /var/db/repos/horcrux/
COPY --from=ctgoverlay / /var/db/repos/canutethegreat/
COPY project/variants/hogsmeade/local-repo/ /var/db/repos/local/
COPY project/variants/gnome-light/packages.txt /tmp/packages-desktop.txt
# sys-apps/horcrux-tools is a fetch-restricted, build-made tarball (see Dockerfile.part2): make it BEFORE `ebuild manifest` and keep a copy in the layer
# (/opt/hxt-dist) because the distfiles cache mount is not part of the image
COPY common/overlay/usr/lib/live-installer /tmp/hxt/usr/lib/live-installer
COPY common/overlay/usr/libexec /tmp/hxt/usr/libexec
COPY common/overlay/usr/bin /tmp/hxt/usr/bin
COPY project/variants/hogshead/overlay/usr/bin/horcrux-orphans /tmp/hxt/usr/bin/horcrux-orphans
COPY project/variants/gnome-light/overlay/usr/share/horcrux/desktop /tmp/hxt/usr/share/horcrux/desktop
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    test -f /usr/include/EGL/egl.h && test -f /usr/include/libudev.h || { echo 'BASE has no development headers: build the gnome-light base with KEEP_DEV=1 (drive.sh)'; exit 1; }; \
    emerge-webrsync -q || emerge --sync -q; \
    eselect profile set default/linux/amd64/23.0/desktop/gnome; \
    export FEATURES="-ccache"; \
    emerge --noreplace --usepkg --getbinpkg app-eselect/eselect-repository dev-vcs/git; \
    eselect repository enable guru; emerge --sync guru; \
    mkdir -p /opt/hxt-dist; tar -C /tmp/hxt --zstd -cf /opt/hxt-dist/horcrux-tools-1.tar.zst .; rm -rf /tmp/hxt; cp /opt/hxt-dist/horcrux-tools-1.tar.zst /var/cache/distfiles/; \
    for e in /var/db/repos/horcrux/sys-kernel/horcrux-hogsmeade-config/*.ebuild /var/db/repos/horcrux/app-admin/rustinion-agent/*.ebuild \
             /var/db/repos/horcrux/sys-apps/horcrux-tools/*.ebuild /var/db/repos/horcrux/sys-kernel/horcrux-unlock/*.ebuild \
             /var/db/repos/horcrux/app-misc/hogsmeade-tools/*.ebuild /var/db/repos/horcrux/app-misc/awccd/*.ebuild \
             /var/db/repos/horcrux/sci-misc/comfyui/*.ebuild /var/db/repos/horcrux/acct-user/comfyui/*.ebuild /var/db/repos/horcrux/acct-group/comfyui/*.ebuild \
             /var/db/repos/canutethegreat/app-misc/alienfx/alienfx-2.4.3.ebuild /var/db/repos/local/sys-apps/hogsmeade-private/*.ebuild /var/db/repos/local/app-misc/hogsmeade-obs/*.ebuild /var/db/repos/local/app-misc/hogsmeade-nextcloud/*.ebuild /var/db/repos/local/app-misc/hogsmeade-speaker-tuning/*.ebuild /var/db/repos/local/app-misc/hogsmeade-input-quirks/*.ebuild; do ebuild "$e" manifest; done
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg=n sys-kernel/horcrux-hogsmeade-config; \
    ls /etc/kernel/config.d; test -s /usr/share/horcrux-hogsmeade/required-config.txt

# --- quick check (minutes): the fragment wins in the final .config, every required option is present
FROM hsm-prep AS hsm-kconfig
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg --getbinpkg sys-libs/binutils-libs app-alternatives/bc sys-devel/bc dev-util/pahole >/dev/null 2>&1; \
    ebuild /var/db/repos/gentoo/sys-kernel/gentoo-kernel/gentoo-kernel-6.18.62.ebuild clean configure > /tmp/kcfg.log 2>&1 || { tail -30 /tmp/kcfg.log; exit 1; }; tail -3 /tmp/kcfg.log; \
    c=$(find /var/tmp/portage/sys-kernel/gentoo-kernel-6.18.62/work -maxdepth 2 -name .config | head -1); echo CONFIG=$c; \
    miss=0; while read -r l; do case "$l" in CONFIG_*) grep -qx "$l" "$c" || { echo "MISSING $l  (have: $(grep -E "^(# )?${l%%=*}[= ]" "$c" | head -1))"; miss=1; };; esac; done < /usr/share/horcrux-hogsmeade/required-config.txt; \
    [ $miss = 0 ] && echo KCONFIG-OK || exit 1

# --- the kernel (about an hour) in its own layer: a later failure does not recompile it
FROM hsm-prep AS hsm-kbuild
# NOTE: `emerge --onlydeps =gentoo-kernel` pulls gentoo-kernel itself in through the virtual/dist-kernel cycle and then dies in
# postinst (dracut cannot install zfs/horcrux-unlock yet), and with --exclude the build deps (bc, pahole) are skipped: install them explicitly. The binpkg dir is a cache mount, so a failure after the
# ~hour-long compile keeps the binary package (the cp at the end copies it into the image layer).
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    --mount=type=cache,id=hs-kernel-binpkgs,target=/var/cache/hs-binpkgs,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/var/cache/hs-binpkgs; \
    emerge --oneshot --usepkg --getbinpkg sys-libs/binutils-libs app-alternatives/bc sys-devel/bc dev-util/pahole > /var/log/hogsmeade-kernel-deps.log 2>&1 || { tail -40 /var/log/hogsmeade-kernel-deps.log; exit 1; }; \
    if ! ls /var/cache/hs-binpkgs/sys-kernel/gentoo-kernel/gentoo-kernel-6.18.62*.gpkg.tar >/dev/null 2>&1; then \
      emerge --buildpkgonly --nodeps --usepkg=n --getbinpkg=n =sys-kernel/gentoo-kernel-6.18.62 \
        > /var/log/hogsmeade-kernel-build.log 2>&1 || { tail -80 /var/log/hogsmeade-kernel-build.log; exit 1; }; \
    fi; \
    mkdir -p /hs-binpkgs && cp -a /var/cache/hs-binpkgs/. /hs-binpkgs/; find /hs-binpkgs -type f | head

# --- desktop (GNOME/Wayland on OpenRC) + the owner's browsers etc., identical steps to variants/desktop (no strip yet)
FROM hsm-prep AS hsm-desktop
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    pk=$(grep -v '^#' /tmp/packages-desktop.txt | tr '\n' ' '); \
    emerge --update --newuse $pk || { rc=$?; \
      echo "== desktop emerge failed (rc=$rc): failing build logs (BuildKit truncates the real compiler error)"; \
      for f in $(ls -t /var/tmp/portage/*/*/temp/build.log 2>/dev/null | head -3); do echo "=== $f"; grep -n -E 'FAILED:|error:|Killed|signal|internal compiler|No space' "$f" | tail -15; tail -30 "$f"; done; free -m | head -2; \
      echo '== retrying once with MAKEOPTS=-j2 (intermittent compile failures, e.g. mutter ninja)'; \
      MAKEOPTS=-j2 emerge --resume || exit $rc; }

# --- everything together
FROM hsm-desktop AS hsm-kinst1
COPY --from=hsm-kbuild /hs-binpkgs /hs-binpkgs
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache" PKGDIR=/hs-binpkgs; \
    echo 'omit_dracutmodules+=" zfs horcrux-unlock "' > /etc/dracut.conf.d/00-hogsmeade-build.conf; \
    emerge --unmerge sys-kernel/gentoo-kernel-bin || true; \
    emerge --oneshot --usepkgonly --nodeps =sys-kernel/gentoo-kernel-6.18.62; \
    ls /lib/modules /boot
FROM hsm-kinst1 AS hsm-kinstall
# the installed kernel tree carries a built objtool; linux-mod-r1 refuses to build modules when its libraries are missing (they were only in the kernel-build layer)
RUN set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg --getbinpkg sys-libs/binutils-libs dev-libs/xxhash virtual/libelf dev-util/pahole app-alternatives/bc sys-devel/bc > /var/log/hogsmeade-objtool-deps.log 2>&1 || { tail -30 /var/log/hogsmeade-objtool-deps.log; exit 1; }; \
    o=/usr/src/linux/tools/objtool/objtool; ldd $o; if ldd $o | grep 'not found'; then echo OBJTOOL-LIBS-MISSING; exit 1; fi
# ZFS and the NVIDIA open modules are built for THAT kernel (never from a cached binpkg of another release)
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    --mount=type=cache,id=hs-kmod-binpkgs,target=/var/cache/hs-kmod-binpkgs,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg" PKGDIR=/var/cache/hs-kmod-binpkgs; \
    # PKGDIR is a cache mount: a failure later in this image keeps the (30 min) module binpkgs. Only valid for THIS kernel binpkg (never reuse after a kernel change).
    emerge --oneshot --usepkg --getbinpkg=n =sys-fs/zfs-kmod-2.3.9 =sys-fs/zfs-2.3.9 x11-drivers/nvidia-drivers \
        > /var/log/hogsmeade-kmods.log 2>&1 || { tail -80 /var/log/hogsmeade-kmods.log; exit 1; }
RUN --mount=type=cache,id=distfiles-${ARCH:-x86_64},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    cp /opt/hxt-dist/horcrux-tools-1.tar.zst /var/cache/distfiles/; \
    rm -f /etc/dracut.conf.d/00-hogsmeade-build.conf; \
    emerge --oneshot sys-kernel/horcrux-unlock sys-apps/horcrux-tools; \
    kr=$(ls /lib/modules | grep -- '-gentoo-dist$' | tail -1); test -n "$kr"; \
    find /lib/modules/$kr -name 'zfs.ko*' | grep .; find /lib/modules/$kr -name 'nvidia.ko*' | grep .; find /lib/modules/$kr -name 'nvidia-drm.ko*' | grep .; \
    for m in vmd snd-soc-rt722-sdca snd-soc-rt1320-sdw iwlwifi r8169 xe alienware-wmi nvidia-wmi-ec-backlight intel-ish-ipc; do \
        find /lib/modules/$kr \( -name "$m.ko*" -o -name "$(echo $m | tr - _).ko*" \) | grep -q . || grep -q -E "/$(echo $m | tr - _)\.ko|/$m\.ko" /lib/modules/$kr/modules.builtin || { echo "MISSING module $m"; exit 1; }; done; \
    depmod $kr; echo $kr > /etc/hogsmeade-kernelrelease

FROM hsm-kinstall AS hsm-final
ARG ARCH=x86_64
# the owner's applications + the HOGSMEADE packages (binhost binpkgs where USE matches, the rest compiles)
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --update --newuse --usepkg --getbinpkg --keep-going \
        sys-firmware/sof-firmware sys-power/switcheroo-control x11-apps/mesa-progs sys-apps/smartmontools sys-apps/nvme-cli app-backup/restic \
        app-containers/docker app-containers/docker-compose app-containers/docker-buildx \
        net-misc/remmina media-gfx/gimp media-gfx/geeqie net-misc/mosh net-analyzer/nmap \
        net-im/discord net-vpn/proton-vpn-gtk-app app-misc/hogsmeade-obs app-misc/hogsmeade-nextcloud app-misc/hogsmeade-speaker-tuning app-misc/hogsmeade-input-quirks app-admin/rustinion-agent sys-firmware/edk2-bin app-admin/sudo \
        sys-process/cronie app-portage/portage-utils app-portage/eix sys-apps/lm-sensors x11-libs/libnotify \
        > /var/log/hogsmeade-apps.log 2>&1 || { tail -60 /var/log/hogsmeade-apps.log; exit 1; }
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --update --newuse --usepkg app-misc/hogsmeade-tools app-misc/awccd app-misc/alienfx \
        > /var/log/hogsmeade-tools.log 2>&1 || { tail -60 /var/log/hogsmeade-tools.log; exit 1; }
# ComfyUI: venv with PyTorch cu130 wheels (large download), own layer so a failure does not redo the above
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache buildpkg"; \
    emerge --oneshot --usepkg sci-misc/comfyui > /var/log/hogsmeade-comfyui.log 2>&1 || { tail -60 /var/log/hogsmeade-comfyui.log; exit 1; }
# private payload (handoff key, agent ssh key, backup credentials) as a machine-local package; the files come from the named context
COPY --from=hsmprivate / /var/db/repos/local/sys-apps/hogsmeade-private/files/
RUN set -e; unset ARCH; export FEATURES="-ccache"; \
    ebuild /var/db/repos/local/sys-apps/hogsmeade-private/hogsmeade-private-1.ebuild manifest; \
    emerge --oneshot sys-apps/hogsmeade-private; \
    rm -rf /var/db/repos/local/sys-apps/hogsmeade-private/files/*; \
    # explicit packages go into @world so `emerge -uDN @world` keeps them current and depclean never removes them
    emerge --noreplace --select=y sys-kernel/gentoo-kernel sys-fs/zfs sys-fs/zfs-kmod x11-drivers/nvidia-drivers sys-kernel/horcrux-unlock \
        sys-kernel/horcrux-hogsmeade-config sys-apps/horcrux-tools app-misc/hogsmeade-tools app-misc/awccd app-misc/alienfx sci-misc/comfyui \
        sys-apps/hogsmeade-private app-misc/hogsmeade-obs app-misc/hogsmeade-nextcloud app-misc/hogsmeade-speaker-tuning app-misc/hogsmeade-input-quirks app-misc/hogsmeade-sudo app-misc/hogsmeade-b2-mounts app-admin/rustinion-agent sys-firmware/sof-firmware sys-kernel/linux-firmware; \
    mkdir -p /usr/share/horcrux/desktop; \
    chmod 644 /usr/share/horcrux/desktop/* 2>/dev/null || true
# desktop markers the installer looks for (the desktop variant ships the same two files), image markers, services; reown.sh then
# puts everything that is still unowned into sys-apps/hogsmeade-image-files
COPY project/variants/gnome-light/overlay/ /
COPY project/variants/hogsmeade/reown.sh /usr/local/sbin/hsm-reown
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    echo "hogsmeade" > /etc/live-iso-variant; \
    for s in hogsmeade-firewall hogsmeade-speaker-tuning awccd nvidia-persistenced switcheroo-control smartd hogsmeade-firstboot comfyui docker; do rc-update add $s default 2>/dev/null || true; done; \
    printf 'DISPLAYMANAGER="gdm"\n' > /etc/conf.d/display-manager; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    bash /usr/local/sbin/hsm-reown 2>&1 | tee /var/log/hogsmeade-reown.log | tail -70; \
    rm -f /usr/local/sbin/hsm-reown; rm -rf /var/tmp/portage /var/cache/distfiles/* 2>/dev/null || true
