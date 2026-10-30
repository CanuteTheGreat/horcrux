# syntax=docker/dockerfile:1.4
# Horcrux "gringotts" variant: the Desktop image (BASE = server rootfs + desktop variant) + the owner's apps + the guided one-step install
# for the ThinkBook 16 G6 + the remote-handoff helpers. EVERY file is owned by an ebuild (verified with qfile -o in the QEMU test):
#   * apps          ::gentoo, GURU (proton-vpn-gtk-app) and the owner's canutethegreat-overlay (brave-bin)
#   * rustinion     app-admin/rustinion-bin from the Horcrux overlay (repo gentoo/)
#   * image files   sys-apps/horcrux-image-files (the live-installer/BE tools/console menu/horcrux-api tree, packed from the build contexts)
#   * helpers       app-misc/gringotts-tools (this variant's local overlay)
#   * PRIVATE       app-misc/gringotts-private (agent SSH public keys + 7-day ephemeral tagged Tailscale key) only when the build
#                   gets a private bundle (named context `priv`, PRIV_DIR in build.sh); never committed, never in a public image.
# Named contexts (build.sh): hxoverlay = repo gentoo/, priv = private bundle, app = the horcrux-api build output image.
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=gringotts

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

# files that came from the repository's overlays (not from a package) are re-packaged as ONE ebuild so Portage owns them:
# the tarball is built from the same build contexts the base layers COPYed (order as in Dockerfile.rootfs: app, project, common, then the desktop variant)
COPY --from=app /out/ /tmp/hxo/app/
COPY project/overlay/ /tmp/hxo/project/
COPY common/overlay/ /tmp/hxo/common/
COPY project/variants/desktop/overlay/ /tmp/hxo/desktop/
COPY project/variants/gringotts/overlay/ /tmp/hxo/gringotts/
COPY project/variants/gringotts/image-files.exclude /tmp/image-files.exclude
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    set -e; unset ARCH; \
    mkdir -p /tmp/imgfiles /tmp/dist; \
    for d in app project common desktop gringotts; do cp -a /tmp/hxo/$d/. /tmp/imgfiles/; done; \
    ( cd /tmp/imgfiles && grep -v '^#' /tmp/image-files.exclude | grep . | while read -r p; do rm -rf "./$p"; done ); \
    find /tmp/imgfiles -name __pycache__ -type d -prune -exec rm -rf {} +; \
    ( cd /tmp/imgfiles && find . \( -type f -o -type l \) | sed 's|^\.||' | xargs -d '\n' qfile -C 2>/dev/null | tee /tmp/already-owned.txt | sed 's/^/NOTE already owned by a package, left out of horcrux-image-files: /'; \
      sed -E 's/.*\(([^)]*)\)$/\1/' /tmp/already-owned.txt | while read -r f; do rm -f "./$f"; done ); \
    tar -C /tmp/imgfiles -cf /tmp/dist/horcrux-image-files-1.tar .; \
    DISTDIR=/tmp/dist ebuild /var/db/repos/horcrux-gringotts/sys-apps/horcrux-image-files/horcrux-image-files-1.ebuild manifest; \
    FEATURES="-collision-protect -protect-owned" COLLISION_IGNORE="/usr /etc /var /opt /lib /lib64 /bin /sbin /boot /root /srv" CONFIG_PROTECT="-*" DISTDIR=/tmp/dist emerge --noreplace --usepkg=n --getbinpkg=n --buildpkg=n sys-apps/horcrux-image-files app-misc/gringotts-tools app-admin/rustinion-bin; \
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
    cd /usr/lib/firmware && rm -rf qcom netronome mellanox mrvl qed dpaa2 liquidio cavium cxgb4 bnx2x bnx2; cd /; \
    find /usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' ! -name locale.alias -exec rm -rf {} + ; \
    find /usr/share/help /usr/share/gnome/help -mindepth 1 -maxdepth 1 ! -name C ! -name 'en*' -exec rm -rf {} + 2>/dev/null || true; \
    find /usr/lib /usr/lib64 -xdev -name '*.a' -type f ! -path '/usr/lib/gcc/*' -delete 2>/dev/null || true; \
    rm -rf /usr/include /usr/lib/llvm/*/include /usr/lib/llvm/*/bin /usr/lib/clang 2>/dev/null || true; \
    echo "desktop" > /etc/live-iso-variant; echo "gringotts" >> /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/db/repos/guru/* /var/db/repos/guru/.git /var/db/repos/canutethegreat-overlay/* /var/db/repos/canutethegreat-overlay/.git \
           /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
