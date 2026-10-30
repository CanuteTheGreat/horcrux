# syntax=docker/dockerfile:1.4
# Horcrux Desktop variant: the finished server rootfs (BASE) + GNOME on Wayland (OpenRC/elogind) + the daily-use tools.
# Binpkgs from the official binhost wherever the USE flags match; the rest compiles (BuildKit cache mounts keep it).
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=desktop

RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; emerge-webrsync -q || emerge --sync -q

COPY project/variants/desktop/portage/ /etc/portage/
COPY project/variants/desktop/repo/ /var/db/repos/horcrux-desktop/
COPY project/variants/desktop/packages.txt /tmp/packages-desktop.txt

# profile with the GNOME defaults (OpenRC flavour: elogind, no systemd), Manifest for the local Brave ebuild, one emerge
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    eselect profile set default/linux/amd64/23.0/desktop/gnome; \
    ( cd /var/db/repos/horcrux-desktop/www-client/brave-bin && ebuild brave-bin-*.ebuild manifest ); \
    export FEATURES="-ccache"; \
    emerge --update --newuse $(grep -v '^#' /tmp/packages-desktop.txt | tr '\n' ' ')

COPY project/variants/desktop/overlay/ /

# the Desktop image marks itself (the installer offers the desktop profile only when this exists); slim down again
# Size: the ISO must stay below the 2 GiB release-asset limit. Firmware for hardware that is not in an x86 laptop/desktop
# (Snapdragon SoCs, server/SmartNICs) is ~900 MB of the uncompressed image; the server image keeps it.
RUN set -e; chmod 644 /usr/share/horcrux/desktop/*; \
    cd /usr/lib/firmware && rm -rf qcom netronome mellanox mrvl qed dpaa2 liquidio cavium cxgb4 bnx2x bnx2; cd /; \
    # English-only message catalogs + emoji dictionaries (~530 MB uncompressed): the image has to fit below 2 GiB
    find /usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' ! -name locale.alias -exec rm -rf {} + ; \
    find /usr/share/ibus/dicts -type f ! -name 'emoji-en*' ! -name 'emoji-ascii*' -delete 2>/dev/null || true; \
    # GNOME help pages for other languages
    find /usr/share/help /usr/share/gnome/help -mindepth 1 -maxdepth 1 ! -name C ! -name 'en*' -exec rm -rf {} + 2>/dev/null || true; \
    # development leftovers of the desktop emerge (~1 GB uncompressed): static libs (libclamav_rust.a 330 MB, QmlDom 218, qpdf 108, LLVM ...),\
    # headers, and the clang/LLVM tools (mesa's llvmpipe/radeonsi only load libLLVM.so, which stays)\
    find /usr/lib /usr/lib64 -xdev -name '*.a' -type f ! -path '/usr/lib/gcc/*' -delete 2>/dev/null || true; \
    rm -rf /usr/include /usr/lib/llvm/*/include /usr/lib/llvm/*/bin /usr/lib/clang 2>/dev/null || true; \
    echo "desktop" > /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
