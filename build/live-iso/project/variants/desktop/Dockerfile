# syntax=docker/dockerfile:1.4
# Horcrux Desktop variant: the finished server rootfs (BASE) + GNOME on Wayland (OpenRC/elogind) + the daily-use tools.
# Binpkgs from the official binhost wherever the USE flags match; the rest compiles (BuildKit cache mounts keep it).
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=desktop

RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    emerge-webrsync -q || emerge --sync -q

COPY project/variants/desktop/portage/ /etc/portage/
COPY project/variants/desktop/repo/ /var/db/repos/horcrux-desktop/
COPY project/variants/desktop/packages.txt /tmp/packages-desktop.txt

# profile with the GNOME defaults (OpenRC flavour: elogind, no systemd), Manifest for the local Brave ebuild, one emerge
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; \
    eselect profile set default/linux/amd64/23.0/desktop/gnome; \
    ( cd /var/db/repos/horcrux-desktop/www-client/brave-bin && ebuild brave-bin-*.ebuild manifest ); \
    export FEATURES="-ccache"; \
    emerge --noreplace $(grep -v '^#' /tmp/packages-desktop.txt | tr '\n' ' ')

COPY project/variants/desktop/overlay/ /

# the Desktop image marks itself (the installer offers the desktop profile only when this exists); slim down again
RUN set -e; chmod 644 /usr/share/horcrux/desktop/*; \
    echo "desktop" > /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
