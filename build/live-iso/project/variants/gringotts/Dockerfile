# syntax=docker/dockerfile:1.4
# Horcrux "gringotts" variant: the Desktop image (BASE = server rootfs + desktop variant) + the owner's apps + the guided one-step install
# for the ThinkBook 16 G6 + the remote-handoff helpers. EVERY file is owned by an ebuild (verified with qfile -o in the QEMU test):
#   * apps          ::gentoo, GURU (proton-vpn-gtk-app) and the owner's canutethegreat-overlay (brave-bin)
#   * rustinion     app-admin/rustinion-bin from the Horcrux overlay (repo gentoo/)
#   * image files   sys-apps/horcrux-tools + horcrux-live-data + sys-kernel/horcrux-unlock (proven on hogshead) + app-misc/gringotts-config (/etc)
#   * helpers       app-misc/gringotts-tools (this variant's local overlay)
#   * PRIVATE       app-misc/gringotts-private (agent SSH public keys + 7-day ephemeral tagged Tailscale key) only when the build
#                   gets a private bundle (named context `priv`, PRIV_DIR in build.sh); never committed, never in a public image.
# Named contexts (build.sh): hxoverlay = minimal Horcrux overlay, hxfull = repo gentoo/ (whole overlay), priv = private bundle.
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=gringotts

# (hxoverlay is deliberately the MINIMAL overlay here (rustinion-bin + metadata): the long header/app layers below stay BuildKit-cached when the
# overlay's other packages change; the full overlay is `hxfull`, copied after them)
COPY --from=hxoverlay / /var/db/repos/horcrux/
COPY project/variants/gringotts/portage/ /etc/portage/

# trees: ::gentoo (the desktop layer deleted it), GURU, the owner's overlay (shallow git clones; pins are printed in the build log)
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    emerge-webrsync -q || emerge --sync -q; \
    git clone -q --depth 1 https://github.com/gentoo-mirror/guru.git /var/db/repos/guru; \
    git clone -q --depth 1 https://github.com/CanuteTheGreat/canutethegreat-overlay.git /var/db/repos/canutethegreat-overlay; \
    for r in guru canutethegreat-overlay; do echo "repo pin: $r $(git -C /var/db/repos/$r rev-parse HEAD)"; done; \
    eselect profile show

COPY project/variants/gringotts/packages.txt /tmp/packages-gringotts.txt

# the Desktop layer deleted /usr/include, the LLVM headers/tools and static libs to fit the ISO; the apps compile (python/C/go): put the headers back by
# re-installing the packages that own them (same cat/pkg:slot, binary packages), then slim again at the end (the final slimming removes them again)
COPY project/variants/gringotts/pkgs-owning-dev.py /tmp/pkgs-owning-dev.py
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    pk=$(python3 /tmp/pkgs-owning-dev.py); echo "re-installing $(echo $pk | wc -w) packages that own headers"; \
    emerge --oneshot --nodeps --usepkg --getbinpkg --buildpkg=n --jobs=6 --keep-going $pk > /tmp/dev.log 2>&1 || grep -E "Failed to emerge" /tmp/dev.log | head; \
    find /etc -name '._cfg*' -delete; rm -f /tmp/dev.log /tmp/pkgs-owning-dev.py; \
    test -f /usr/include/stdio.h && ls /usr/include/python3*/Python.h >/dev/null

# apps: Brave comes from the owner's overlay (re-merged below so the package belongs to canutethegreat-overlay, not the desktop layer's stop-gap copy)
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --update --newuse --deep $(grep -v '^#' /tmp/packages-gringotts.txt | tr '\n' ' '); \
    emerge --oneshot --usepkg=n --getbinpkg=n www-client/brave-bin::canutethegreat-overlay; \
    rm -rf /var/db/repos/horcrux-desktop /etc/portage/repos.conf/horcrux-desktop.conf

# this variant's local overlay is copied only now (editing it must not invalidate the long header/app layers above)
COPY project/variants/gringotts/repo/ /var/db/repos/horcrux-gringotts/

# Ownership of the image's own files: the packages PROVEN on hogshead (overlay gentoo/ = repo horcrux): sys-kernel/horcrux-unlock (dracut one-passphrase),
# sys-apps/horcrux-tools (installer, live scripts, horcrux-be, horcrux-orphans), sys-apps/horcrux-live-data (branding + portage metadata snapshot),
# app-admin/rustinion-bin. The two tools/data tarballs are fetch-restricted distfiles made from this build's own trees (nothing is downloaded);
# the loose copies from the ISO overlay are removed first so Portage owns the files. The gringotts tty1 menu is this variant's
# live-installer-session, packed over the common one inside the tools tarball.
COPY --from=hxfull / /var/db/repos/horcrux/
COPY project/variants/gringotts/portage-late/ /etc/portage/
COPY common/overlay/usr/lib/live-installer /tmp/hxt/usr/lib/live-installer
COPY common/overlay/usr/libexec /tmp/hxt/usr/libexec
COPY common/overlay/usr/bin /tmp/hxt/usr/bin
COPY project/variants/hogshead/overlay/usr/bin/horcrux-orphans /tmp/hxt/usr/bin/horcrux-orphans
COPY project/variants/desktop/overlay/usr/share/horcrux/desktop /tmp/hxt/usr/share/horcrux/desktop
COPY project/variants/gringotts/overlay/usr/libexec/ /tmp/hxt/usr/libexec/
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    tar -C /tmp/hxt --zstd -cf /var/cache/distfiles/horcrux-tools-1.tar.zst .; rm -rf /tmp/hxt; \
    tar -C / --zstd -cf /var/cache/distfiles/horcrux-live-data-1.tar.zst usr/share/live-installer; \
    for e in sys-apps/horcrux-live-data sys-apps/horcrux-tools sys-kernel/horcrux-unlock app-admin/rustinion-bin; do ebuild /var/db/repos/horcrux/$e/*.ebuild manifest; done; \
    rm -rf /usr/bin/horcrux /usr/bin/horcrux-api /usr/share/horcrux/ui /etc/init.d/horcrux-api /etc/init.d/horcrux-guests /etc/conf.d/horcrux-api \
           /usr/share/horcrux/t2 /usr/lib/live-installer /usr/libexec/live-* /usr/libexec/console-menu-session /usr/bin/live-installer /usr/bin/console-menu \
           /usr/bin/live-persist /usr/bin/live-rescue /usr/bin/horcrux-be /usr/bin/horcrux-t2-firmware /usr/share/horcrux/desktop \
           /usr/share/live-installer /usr/lib/dracut/modules.d/91horcrux-unlock /etc/init.d/t2fanrd /etc/init.d/tiny-dfr \
           /usr/share/tiny-dfr /usr/lib/udev/rules.d/99-touchbar-tiny-dfr.rules /usr/lib/udev/rules.d/99-touchbar-seat.rules /usr/bin/t2fanrd /usr/bin/tiny-dfr; \
    rm -f /etc/runlevels/*/horcrux-api /etc/runlevels/*/horcrux-guests /etc/runlevels/*/t2fanrd /etc/runlevels/*/tiny-dfr; \
    emerge --oneshot --usepkg=n --getbinpkg=n sys-kernel/horcrux-unlock sys-apps/horcrux-tools sys-apps/horcrux-live-data; \
    emerge --noreplace --usepkg=n --getbinpkg=n app-admin/rustinion-bin; \
    emerge --noreplace --select=y sys-kernel/horcrux-unlock sys-apps/horcrux-tools sys-apps/horcrux-live-data app-admin/rustinion-bin; \
    printf 'desktop\ngringotts\n' > /etc/live-iso-variant

# the image's remaining /etc files (init scripts, profile.d, dracut/Portage config, markers: the repository's overlay /etc trees and the build-time Portage
# configuration) are ONE small ebuild, gringotts-config; usr/ content is owned by the packages above, so it is excluded from the tarball
COPY project/overlay/ /tmp/hxo/project/
COPY common/overlay/ /tmp/hxo/common/
COPY project/variants/gringotts/image-files.exclude /tmp/image-files.exclude
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    set -e; unset ARCH; \
    mkdir -p /tmp/imgfiles /tmp/dist; \
    for d in project common; do ( cd /tmp/hxo/$d && cp -a etc /tmp/imgfiles/ ) ; done; \
    ( cd /tmp/imgfiles && grep -v '^#' /tmp/image-files.exclude | grep . | while read -r p; do rm -rf "./$p"; done ); \
    ( cd / && cp -a --parents etc/portage/make.conf etc/portage/make.conf.arch etc/portage/binrepos.conf etc/portage/env etc/portage/package.* etc/portage/repos.conf \
        etc/sudoers.d/wheel etc/live-installer/live-only-packages etc/live-iso-label etc/live-iso-volid etc/live-iso-variant /tmp/imgfiles/ ); \
    ( cd /tmp/imgfiles && find . \( -type f -o -type l \) | sed 's|^\.||' | xargs -d '\n' qfile -C 2>/dev/null | tee /tmp/already-owned.txt | sed 's/^/NOTE already owned by a package, left out of gringotts-config: /'; \
      sed -E 's/.*\(([^)]*)\)$/\1/' /tmp/already-owned.txt | while read -r f; do rm -f "./$f"; done ); \
    tar -C /tmp/imgfiles -cf /tmp/dist/gringotts-config-1.tar .; \
    DISTDIR=/tmp/dist ebuild /var/db/repos/horcrux-gringotts/app-misc/gringotts-config/gringotts-config-1.ebuild manifest; \
    FEATURES="-collision-protect -protect-owned" COLLISION_IGNORE="/etc" CONFIG_PROTECT="-*" DISTDIR=/tmp/dist emerge --noreplace --usepkg=n --getbinpkg=n --buildpkg=n app-misc/gringotts-config app-misc/gringotts-tools; \
    rm -rf /tmp/dist /tmp/hxo /tmp/imgfiles /tmp/image-files.exclude /tmp/already-owned.txt

# PRIVATE bundle (agent SSH public keys + Tailscale key): absent in a public build. DISTDIR is a tmpfs-like scratch dir that is deleted,
# the binary package is not built, so nothing secret reaches the binpkg/distfile caches.
RUN --mount=type=bind,from=priv,target=/priv \
    set -e; unset ARCH; \
    if [ -s /priv/ts-authkey ] && [ -s /priv/authorized_keys ]; then \
      mkdir -p /tmp/dist; tar -C /priv -cf /tmp/dist/gringotts-private-1.tar authorized_keys ts-authkey; \
      DISTDIR=/tmp/dist ebuild /var/db/repos/horcrux-gringotts/app-misc/gringotts-private/gringotts-private-1.ebuild manifest; \
      CONFIG_PROTECT="-*" DISTDIR=/tmp/dist emerge --noreplace --usepkg=n --getbinpkg=n --buildpkg=n app-misc/gringotts-private; \
      rm -rf /tmp/dist; echo "PRIVATE bundle installed (app-misc/gringotts-private)"; \
    else echo "no private bundle: PUBLIC image (no SSH keys, no Tailscale key)"; fi

# package-owned config files that are only edited (not shipped by gringotts-tools: iwd and openrc own them): iwd must not do its own DHCP (NetworkManager does),
# the live stick has its own host name (the installer sets the real one on the target)
RUN printf '[General]\nEnableNetworkConfiguration=false\n' > /etc/iwd/main.conf; printf 'hostname="gringotts-live"\n' > /etc/conf.d/hostname

# runlevels of the live stick (and of the install, which copies them): NetworkManager (iwd backend; nmtui) instead of dhcpcd; the gringotts tty1 menu comes from the live-installer-session override in ./overlay.
RUN set -e; \
    rc-update del dhcpcd default 2>/dev/null || true; \
    rc-update add dbus default; rc-update add NetworkManager default; rc-update add iwd default; \
    test -x /usr/bin/nmtui && test -x /usr/bin/gringotts-install; \
    chmod 700 /usr/share/gringotts 2>/dev/null || true

# slim again: the apps pulled the full firmware/locale/doc/static-lib sets back in; Portage keeps the records, the files are just not shipped
RUN set -e; \
    # build-time tools the apps left behind (BDEPEND only): unmerge them properly (depclean refuses anything still needed), then ship the ISO under Forgejo's 2 GiB asset limit
    emerge --depclean --with-bdeps=n dev-lang/rust-bin dev-lang/go dev-util/rustup app-eselect/eselect-rust > /tmp/depclean-tools.log 2>&1 || true; tail -12 /tmp/depclean-tools.log; rm -f /tmp/depclean-tools.log; \
    cd /usr/lib/firmware && rm -rf qcom netronome mellanox mrvl qed dpaa2 liquidio cavium cxgb4 bnx2x bnx2 \
        nvidia amdgpu radeon mediatek ath10k ath11k ath12k rtw89 brcm qca; cd /; \
    find /usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' ! -name locale.alias -exec rm -rf {} + ; \
    find /usr/share/help /usr/share/gnome/help -mindepth 1 -maxdepth 1 ! -name C ! -name 'en*' -exec rm -rf {} + 2>/dev/null || true; \
    find /usr/lib /usr/lib64 -xdev -name '*.a' -type f ! -path '/usr/lib/gcc/*' -delete 2>/dev/null || true; \
    rm -rf /usr/include /usr/lib/llvm/*/include /usr/lib/llvm/*/bin /usr/lib/clang 2>/dev/null || true; \
    rm -rf /var/db/repos/gentoo/* /var/db/repos/guru/* /var/db/repos/guru/.git /var/db/repos/canutethegreat-overlay/* /var/db/repos/canutethegreat-overlay/.git \
           /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
