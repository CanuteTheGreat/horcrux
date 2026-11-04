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

# profile with the GNOME defaults (OpenRC flavour: elogind, no systemd), one emerge (the owner's apps are NOT here: machine layers)
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    eselect profile set default/linux/amd64/23.0/desktop/gnome; \
    export FEATURES="-ccache"; \
    emerge --update --newuse $(grep -v '^#' /tmp/packages-desktop.txt | tr '\n' ' ')

# GNOME under OpenRC (patches in /etc/portage/patches/gnome-base/gnome-session-openrc): the gsd meta services lose supervisor=supervise-daemon (openrc refused to start
# them: no session), and gsd-media-keys/gsd-sound `want` + start after wireplumber and pipewire-pulse (nothing started PipeWire in the user session:
# pipewire.desktop is X-GNOME-HiddenUnderSystemd; and a gsd that came up before it never reconnected: dead volume keys). Binary packages ignore patches: rebuild from source.
RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; export FEATURES="-ccache"; \
    emerge --oneshot --usepkg=n --getbinpkg=n --buildpkg=n gnome-base/gnome-session-openrc; \
    if grep -q supervise-daemon /etc/user/init.d/gnome-settings-daemon-wayland /etc/user/init.d/gnome-settings-daemon-x11; then echo 'gsd supervisor patch not applied'; exit 1; fi; \
    grep -q 'want pipewire-pulse wireplumber' /etc/user/init.d/gsd || { echo 'gsd audio ordering patch not applied'; exit 1; }; \
    grep -q 'keep the shell there' /etc/user/init.d/gnome-shell-wayland || { echo 'gnome-shell user-bus patch not applied'; exit 1; }

# GNOME system-wide defaults (every GNOME install gets them through @world): AppIndicator tray icons enabled in a packaged system dconf db. The Horcrux
# overlay is copied only now so editing it does not invalidate the long layers above.
COPY --from=hxfull / /var/db/repos/horcrux/
# the [horcrux] repos.conf entry is package-owned (sys-apps/horcrux-repo-config, a dependency of horcrux-gnome-defaults: git sync from the overlay's own Forgejo repo,
# auto-sync yes); it is placed once from the package's own file so this emerge can see the overlay, and the package then owns it
RUN set -e; unset ARCH; export FEATURES="-ccache"; \
    install -D -m644 /var/db/repos/horcrux/sys-apps/horcrux-repo-config/files/horcrux.conf /etc/portage/repos.conf/horcrux.conf; \
    emerge --update --newuse --usepkg=n --getbinpkg=n --select gnome-extra/horcrux-gnome-defaults; \
    test -s /etc/dconf/db/local && test -f /etc/dconf/profile/user && qfile -q /etc/dconf/db/local && qfile -q /etc/portage/repos.conf/horcrux.conf; \
    # keyring PAM: one pam_gnome_keyring set (gdm-password), session after elogind; postinst of horcrux-gnome-defaults-1-r2 fixes pambase's duplicate. Fails the build on a stale base.
    /usr/libexec/horcrux-keyring-pam check && qfile -q /usr/libexec/horcrux-keyring-pam /usr/bin/horcrux-keyring-status; \
    # the shipped tree is a git checkout (snapshot of this build's overlay) so `emaint sync -r horcrux` fast-forwards it to the published overlay (sync-depth 1 = fetch + reset)\
    cd /var/db/repos/horcrux; git init -q -b main; git add -A; { git diff --cached --quiet || git -c user.name=horcrux-image -c user.email=image@horcrux.invalid commit -q -m "image snapshot (desktop edition)"; }; \
    git remote get-url origin >/dev/null 2>&1 || git remote add origin "$(sed -n 's/^sync-uri *= *//p' /etc/portage/repos.conf/horcrux.conf)"; git rev-parse --short HEAD

COPY project/variants/gnome-light/overlay/ /

COPY common/vdb-prune.py /tmp/vdb-prune.py
COPY project/public-forbidden.txt /tmp/public-forbidden.txt

# the Desktop image marks itself (the installer offers the desktop profile only when this exists); slim down again
# Size: the ISO must stay below the 2 GiB release-asset limit. Firmware for hardware that is not in an x86 laptop/desktop
# (Snapdragon SoCs, server/SmartNICs) is ~900 MB of the uncompressed image; the server image keeps it.
RUN set -e; chmod 644 /usr/share/horcrux/desktop/*; \
    # GATE: a public desktop edition carries no personal / machine-specific package (project/public-forbidden.txt)
    for p in $(grep -v '^#' /tmp/public-forbidden.txt); do if qlist -I "$p" | grep .; then echo "PERSONAL/MACHINE PACKAGE $p IN A PUBLIC IMAGE"; exit 1; fi; done; rm -f /tmp/public-forbidden.txt; \
    # GATE: the desktop edition sits on the core (every shared fix lives there, not in the GNOME package)
    qlist -I sys-apps/horcrux-core net-misc/horcrux-time net-misc/horcrux-chrony-nm sys-apps/horcrux-firstboot sys-apps/horcrux-backup | wc -l | grep -qx 5; \
    # build-only toolchains the source rebuilds (gsd patches, horcrux-gnome-defaults) can leave behind: depclean removes them only when nothing
    # in @world/runtime deps needs them (rust-bin alone is ~1 GB uncompressed); never fails the build
    emerge --depclean --with-bdeps=n dev-lang/rust-bin dev-lang/go dev-util/rustup app-eselect/eselect-rust > /tmp/depclean-tools.log 2>&1 || true; tail -8 /tmp/depclean-tools.log; rm -f /tmp/depclean-tools.log; \
    cd /usr/lib/firmware && rm -rf qcom netronome mellanox mrvl qed dpaa2 liquidio cavium cxgb4 bnx2x bnx2; cd /; \
    # English-only message catalogs + emoji dictionaries (~530 MB uncompressed): the image has to fit below 2 GiB
    find /usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' ! -name locale.alias -exec rm -rf {} + ; \
    find /usr/share/ibus/dicts -type f ! -name 'emoji-en*' ! -name 'emoji-ascii*' -delete 2>/dev/null || true; \
    # GNOME help pages for other languages
    find /usr/share/help /usr/share/gnome/help -mindepth 1 -maxdepth 1 ! -name C ! -name 'en*' -exec rm -rf {} + 2>/dev/null || true; \
    # development leftovers of the desktop emerge (~1 GB uncompressed): static libs (libclamav_rust.a 330 MB, QmlDom 218, qpdf 108, LLVM ...),\
    # headers, and the clang/LLVM tools (mesa's llvmpipe/radeonsi only load libLLVM.so, which stays)\
    # KEEP the toolchain's own development files (glibc + kernel headers + the libraries kernel-module builds link: elfutils, openssl, zlib, zstd, xz, ncurses):\
    # stripping them left every installed system unable to compile anything (gringotts, 2026-11-03: `C compiler cannot create executables`, missing\
    # libc_nonshared.a/stdio.h), which also breaks the zfs-kmod rebuild on every kernel update. ~100 MB; the rest of the headers/static libs go.\
    qlist -e sys-libs/glibc sys-kernel/linux-headers dev-libs/elfutils dev-libs/openssl sys-libs/zlib app-arch/zstd app-arch/xz-utils sys-libs/ncurses sys-libs/libxcrypt | grep -E '^/usr/(include|lib64?)/' | sort -u > /tmp/devkeep.lst; \
    test "$(wc -l < /tmp/devkeep.lst)" -gt 500; \
    find /usr/lib /usr/lib64 -xdev -name '*.a' -type f ! -path '/usr/lib/gcc/*' 2>/dev/null | grep -vxFf /tmp/devkeep.lst | xargs -r rm -f; \
    find /usr/include -xdev \( -type f -o -type l \) 2>/dev/null | grep -vxFf /tmp/devkeep.lst | xargs -r rm -f; find /usr/include -xdev -depth -type d -empty -delete 2>/dev/null || true; \
    rm -rf /usr/lib/llvm/*/include /usr/lib/llvm/*/bin /usr/lib/clang 2>/dev/null || true; \
    # gate: the toolchain still builds a C program that needs libc_nonshared.a and the kernel headers\
    printf '#include <stdio.h>\n#include <linux/types.h>\nint main(void){puts("ok");return 0;}\n' > /tmp/t.c && gcc /tmp/t.c -o /tmp/t && /tmp/t && rm -f /tmp/t /tmp/t.c /tmp/devkeep.lst; \
    echo "desktop" > /etc/live-iso-variant; echo "gnome-light" >> /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    # Portage's records match what was stripped on purpose (the reviewed list in common/vdb-prune.py); anything else missing fails the package-contents gate
    python3 /tmp/vdb-prune.py && rm -f /tmp/vdb-prune.py
