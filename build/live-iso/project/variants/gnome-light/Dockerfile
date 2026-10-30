# syntax=docker/dockerfile:1.4
# Horcrux gnome-light variant (was `desktop`; name = <product>-<DE>-<init>, see docs/ISO-INSTALL.md "Image names"): the finished server rootfs (BASE) + GNOME on Wayland (OpenRC/elogind) + the daily-use tools.
# Binpkgs from the official binhost wherever the USE flags match; the rest compiles (BuildKit cache mounts keep it).
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG VARIANT=gnome-light

RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; emerge-webrsync -q || emerge --sync -q

COPY project/variants/gnome-light/portage/ /etc/portage/
COPY project/variants/gnome-light/repo/ /var/db/repos/horcrux-gnome-light/
COPY project/variants/gnome-light/packages.txt /tmp/packages-desktop.txt

# profile with the GNOME defaults (OpenRC flavour: elogind, no systemd), the owner's overlay (Brave), one emerge
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    eselect profile set default/linux/amd64/23.0/desktop/gnome; \
    git clone -q --depth 1 https://github.com/CanuteTheGreat/canutethegreat-overlay.git /var/db/repos/canutethegreat-overlay; \
    export FEATURES="-ccache"; \
    emerge --update --newuse $(grep -v '^#' /tmp/packages-desktop.txt | tr '\n' ' ')

# GNOME under OpenRC (patches in /etc/portage/patches/gnome-base/gnome-session-openrc): the gsd meta services lose supervisor=supervise-daemon (openrc refused to start
# them: no session), and gsd-media-keys/gsd-sound `want` + start after wireplumber and pipewire-pulse (nothing started PipeWire in the user session:
# pipewire.desktop is X-GNOME-HiddenUnderSystemd; and a gsd that came up before it never reconnected: dead volume keys). Binary packages ignore patches: rebuild from source.
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg=n --getbinpkg=n --buildpkg=n gnome-base/gnome-session-openrc; \
    if grep -q supervise-daemon /etc/user/init.d/gnome-settings-daemon-wayland /etc/user/init.d/gnome-settings-daemon-x11; then echo 'gsd supervisor patch not applied'; exit 1; fi; \
    grep -q 'want pipewire-pulse wireplumber' /etc/user/init.d/gsd || { echo 'gsd audio ordering patch not applied'; exit 1; }

COPY project/variants/gnome-light/overlay/ /

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
    echo "desktop" > /etc/live-iso-variant; echo "gnome-light" >> /etc/live-iso-variant; \
    # the overlay tree is re-cloned by `emaint sync` (repos.conf stays); ship an EMPTY dir, not a stale copy
    rm -rf /var/db/repos/canutethegreat-overlay; mkdir -p /var/db/repos/canutethegreat-overlay; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
