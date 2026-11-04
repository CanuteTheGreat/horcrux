# syntax=docker/dockerfile:1.4
# Horcrux CORE layer (applied by build.sh to EVERY Horcrux image, the headless server included, before any desktop edition or machine layer):
# the server-first base. Layering (owner 2026-11-03: "It should work on a headless server too. It shouldn't be a desktop-only OS."):
#   core            this layer = sys-apps/horcrux-core (clock rules + chrony + the NetworkManager/dhcpcd clock hooks, first-boot network wait, overlay
#                   sync config, backup, ZFS dist-kernel rule). NO display stack, NO desktop, NO machine-specific application.
#   desktop edition project/variants/gnome-light (GNOME) FROM this image: only adds the desktop; its package gnome-extra/horcrux-gnome-defaults depends on core
#   machine layers  project/variants/{gringotts,hogshead,hogsmeade,...}: the owner's own apps on top of an edition
# The overlay packages are always built from this tree (--usepkg-exclude: a cached binary package of an older build of the same version is never reused); everything else uses the binhost.
# Idempotent: applying it to an image that already has the core (a REUSE_ROOTFS base) is a no-op.
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=core

RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; if qlist -I sys-apps/horcrux-core >/dev/null 2>&1; then echo "core already installed"; else emerge-webrsync -q || emerge --sync -q; fi

# the overlay is copied only now: editing it does not invalidate the long layers above
COPY --from=hxfull / /var/db/repos/horcrux/
COPY common/vdb-prune.py /tmp/vdb-prune.py
COPY project/public-forbidden.txt /tmp/public-forbidden.txt

RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    if qlist -I sys-apps/horcrux-core >/dev/null 2>&1; then echo "core already installed: nothing to do"; rm -f /tmp/vdb-prune.py /tmp/public-forbidden.txt; exit 0; fi; \
    # the [horcrux] repos.conf entry is package-owned (sys-apps/horcrux-repo-config, a dependency of the core); placed once from the package's own file so this emerge sees the overlay
    install -D -m644 /var/db/repos/horcrux/sys-apps/horcrux-repo-config/files/horcrux.conf /etc/portage/repos.conf/horcrux.conf; \
    qlist -I | sort > /tmp/pkgs.before; \
    emerge --update --newuse --usepkg-exclude "sys-apps/horcrux-* net-misc/horcrux-*" --select sys-apps/horcrux-core > /tmp/core-emerge.log 2>&1 || { tail -60 /tmp/core-emerge.log; exit 1; }; \
    qlist -I | sort > /tmp/pkgs.after; comm -13 /tmp/pkgs.before /tmp/pkgs.after > /tmp/pkgs.added; \
    echo "core added $(wc -l < /tmp/pkgs.added) packages:"; cat /tmp/pkgs.added; \
    # GATE: the headless core pulls no display stack (the live-only graphical installer stack was there before this layer and is removed again on a server install)
    if grep -E '^(gui-libs|gui-wm|x11-base|x11-libs|x11-misc|x11-apps|x11-drivers|x11-themes|gnome-base|gnome-extra|kde-|dev-qt|media-libs/mesa|media-libs/libglvnd|xfce-)' /tmp/pkgs.added; then echo "CORE PULLED A DISPLAY STACK (see above): core must stay headless"; exit 1; fi; \
    # GATE: nothing personal / machine specific in a public image (project/public-forbidden.txt: the owner's apps live in the machine layers)
    for p in $(grep -v '^#' /tmp/public-forbidden.txt); do if qlist -I "$p" | grep .; then echo "PERSONAL/MACHINE PACKAGE $p IN A PUBLIC IMAGE"; exit 1; fi; done; \
    # the clock rules were applied by net-misc/horcrux-time's postinst; every file is package-owned
    horcrux-time-config check; \
    qfile -q /usr/sbin/horcrux-time-config /usr/libexec/horcrux-timesync /etc/init.d/horcrux-timesync /etc/init.d/horcrux-firstboot /usr/libexec/horcrux-firstboot \
              /etc/NetworkManager/dispatcher.d/20-horcrux-chrony /etc/portage/package.use/horcrux-core /etc/portage/repos.conf/horcrux.conf /usr/sbin/ctg-restic /usr/share/horcrux/core-release; \
    # services: time sync + first boot + cron (the backup job) in the image's default runlevel; the installer enables them on the installed system too (profile.toml)
    for s in chronyd horcrux-timesync horcrux-firstboot cronie; do rc-update add $s default; done; \
    # the shipped overlay tree is a git checkout (snapshot of this build) so `emaint sync -r horcrux` fast-forwards it to the published overlay
    cd /var/db/repos/horcrux; git init -q -b main; git add -A; git -c user.name=horcrux-image -c user.email=image@horcrux.invalid commit -q -m "image snapshot (core)"; \
    git remote add origin "$(sed -n 's/^sync-uri *= *//p' /etc/portage/repos.conf/horcrux.conf)"; \
    # upstream tracking ref: without it `emaint sync -r horcrux` fails (Portage's git sync asks for @{upstream}); gate: the next line fails the build otherwise
    git config branch.main.remote origin; git config branch.main.merge refs/heads/main; git update-ref refs/remotes/origin/main HEAD; git rev-parse --abbrev-ref '@{upstream}' | grep -qx origin/main; cd /; \
    # same slimming the base image got (docs, the tree, distfiles): the package database is made to match what was removed
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /var/tmp/portage /var/log/emerge* /tmp/core-emerge.log /tmp/pkgs.* /tmp/public-forbidden.txt 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    python3 /tmp/vdb-prune.py && rm -f /tmp/vdb-prune.py

