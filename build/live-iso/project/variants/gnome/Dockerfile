# syntax=docker/dockerfile:1.4
# Horcrux gnome variant = the FULL GNOME desktop edition (os-release VARIANT "GNOME"): the gnome-light layer (BASE; build.sh chains it through `requires`)
# + gnome-base/gnome with its default apps (gnome-extra/horcrux-gnome-extras: gnome-extra-apps minus Epiphany and the apps that need systemd).
# The public small option stays variants/gnome-light. The owner's own machine layers (hogsmeade, hogshead, gringotts) install the same packages.txt.
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
# init system of the image + the per-arch-and-init binary package cache (build.sh passes both; see common/Dockerfile.rootfs)
ARG INIT=openrc
ARG BINPKG_ID=binpkgs-${ARCH}
ARG VARIANT=gnome

COPY project/variants/gnome/portage/ /etc/portage/
COPY project/variants/gnome/packages.txt /tmp/packages-gnome.txt
# the extras meta package lives in the Horcrux overlay (hxfull = the repo's gentoo/)
COPY --from=hxfull /gnome-extra/horcrux-gnome-extras/ /var/db/repos/horcrux/gnome-extra/horcrux-gnome-extras/

RUN --mount=type=cache,id=${BINPKG_ID},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge-webrsync -q || emerge --sync -q; \
    pk=$(grep -v '^#' /tmp/packages-gnome.txt | tr '\n' ' '); \
    emerge --update --newuse --deep $pk || { rc=$?; \
      echo "== gnome emerge failed (rc=$rc): failing build logs"; \
      for f in $(ls -t /var/tmp/portage/*/*/temp/build.log 2>/dev/null | head -3); do echo "=== $f"; tail -30 "$f"; done; \
      MAKEOPTS=-j2 emerge --resume || exit $rc; }; \
    # GATE: the full desktop is there, Epiphany is not, the edition is still named GNOME
    qlist -I gnome-base/gnome gnome-extra/horcrux-gnome-extras | wc -l | grep -qx 2; \
    for a in gnome-extra/gnome-calendar gnome-extra/gnome-contacts sci-geosciences/gnome-maps gnome-extra/gnome-weather gnome-extra/gnome-clocks media-sound/gnome-music app-editors/gnome-text-editor sys-apps/baobab net-misc/gnome-connections media-video/totem app-text/papers media-gfx/loupe; do qlist -I "$a" | grep -q . || { echo "MISSING from the full GNOME edition: $a"; exit 1; }; done; \
    ! qlist -I www-client/epiphany | grep .; \
    grep -qx 'VARIANT_ID=gnome' /usr/lib/os-release; \
    echo "gnome" >> /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/tmp/portage /tmp/packages-gnome.txt 2>/dev/null || true
