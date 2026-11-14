# syntax=docker/dockerfile:1.4
# Horcrux gnome-light variant (was `desktop`; name = <product>-<DE>-<init>, see docs/ISO-INSTALL.md "Image names"): the finished server rootfs (BASE) + GNOME on Wayland (OpenRC/elogind) + the daily-use tools.
# Binpkgs from the official binhost wherever the USE flags match; the rest compiles (BuildKit cache mounts keep it).
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
# init system of the image + the per-arch-and-init binary package cache (build.sh passes both; see common/Dockerfile.rootfs)
ARG INIT=openrc
ARG BINPKG_ID=binpkgs-${ARCH}
ARG VARIANT=gnome-light

RUN --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    unset ARCH; emerge-webrsync -q || emerge --sync -q

COPY project/variants/gnome-light/portage/ /etc/portage/
COPY project/variants/gnome-light/repo/ /var/db/repos/horcrux-gnome-light/
COPY project/variants/gnome-light/packages.txt /tmp/packages-desktop.txt

# profile with the GNOME defaults for the image's init (OpenRC: desktop/gnome + elogind; systemd: desktop/gnome/systemd, upstream gnome-session),
# one emerge (the owner's apps are NOT here: machine layers). Lines `openrc? atom` / `systemd? atom` in packages.txt are for that init only.
RUN --mount=type=cache,id=${BINPKG_ID},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; unset ARCH; \
    case "${INIT}" in systemd) rm -f /etc/portage/package.use/init-openrc; eselect profile set default/linux/amd64/23.0/desktop/gnome/systemd ;; \
      *) rm -f /etc/portage/package.use/init-systemd; eselect profile set default/linux/amd64/23.0/desktop/gnome ;; esac; \
    export FEATURES="-ccache"; \
    pk=$(sed -E -e 's/#.*//' -e "/^(openrc|systemd)\? /{/^${INIT}\? /!d; s/^${INIT}\? //}" /tmp/packages-desktop.txt | tr '\n' ' '); \
    emerge --update --newuse $pk || { rc=$?; \
      echo "== desktop emerge failed (rc=$rc): failing build logs (BuildKit truncates the real compiler error)"; \
      for f in $(ls -t /var/tmp/portage/*/*/temp/build.log 2>/dev/null | head -3); do echo "=== $f"; grep -n -E 'FAILED:|error:|Killed|signal|internal compiler|No space' "$f" | tail -15; tail -30 "$f"; done; free -m | head -2; \
      echo '== retrying once with MAKEOPTS=-j2 (intermittent compile failures, e.g. mutter ninja)'; \
      MAKEOPTS=-j2 emerge --resume || exit $rc; }

# Mutter's ScreenCast frame limiter (patch in /etc/portage/patches/x11-wm/mutter, copied above with the rest of portage/; --usepkg-exclude in make.conf made the
# desktop emerge build mutter from source): remote desktop / screen sharing delivered 30-40 fps at a 60 Hz display. Fail the build when the patch was not applied.
RUN set -e; strings /usr/lib64/libmutter-*.so.0 | grep -q 'too early (nearest interval boundary)' || { echo 'mutter ScreenCast frame limiter patch not applied (binary package used?)'; exit 1; }

# GNOME system-wide defaults (every GNOME install gets them through @world): AppIndicator tray icons enabled in a packaged system dconf db. The Horcrux
# overlay is copied only now so editing it does not invalidate the long layers above.
COPY --from=hxfull / /var/db/repos/horcrux/
# the [horcrux] repos.conf entry is package-owned (sys-apps/horcrux-repo-config, a dependency of horcrux-gnome-defaults: git sync from the overlay's own Forgejo repo,
# auto-sync yes); it is placed once from the package's own file so this emerge can see the overlay, and the package then owns it
RUN set -e; unset ARCH; export FEATURES="-ccache"; \
    install -D -m644 /var/db/repos/horcrux/sys-apps/horcrux-repo-config/files/horcrux.conf /etc/portage/repos.conf/horcrux.conf; \
    emerge --update --newuse --usepkg=n --getbinpkg=n --select gnome-extra/horcrux-gnome-defaults; \
    # GNOME under OpenRC = gnome-base/gnome-session-openrc-49.2-r2::horcrux (pulled by horcrux-gnome-defaults; its patches used to be /etc/portage/patches in this image):\
    # gsd meta services without a supervisor, gsd audio after PipeWire, gnome-shell on the user bus, and session-scoped user services (a GNOME logout stops the\
    # session's services even while an ssh login of the user keeps the OpenRC user runlevel alive; no gnome-shell respawn into a closed session)\
    # systemd image: upstream gnome-session (systemd user units, systemd-logind) as on any systemd distribution; gnome-session-openrc must not be there
    if [ "${INIT}" = systemd ]; then qlist -I gnome-base/gnome-session; ! qlist -I gnome-base/gnome-session-openrc; ! qlist -I sys-auth/elogind; \
      test -f /usr/lib/systemd/user/gnome-session@.target || test -f /usr/lib/systemd/user/gnome-session-wayland@.target; \
    else \
    qlist -IRv gnome-base/gnome-session-openrc | grep -q '::horcrux'; test ! -e /etc/portage/patches/gnome-base/gnome-session-openrc; \
    if grep -q supervise-daemon /etc/user/init.d/gnome-settings-daemon-wayland /etc/user/init.d/gnome-settings-daemon-x11; then echo 'gsd supervisor patch not applied'; exit 1; fi; \
    grep -q 'want pipewire-pulse wireplumber' /etc/user/init.d/gsd || { echo 'gsd audio ordering patch not applied'; exit 1; }; \
    grep -q 'keep the shell there' /etc/user/init.d/gnome-shell-wayland || { echo 'gnome-shell user-bus patch not applied'; exit 1; }; \
    test -x /usr/libexec/gnome-session-openrc-helper && grep -q 'command_args="guard /usr/bin/gnome-shell' /etc/user/init.d/gnome-shell-wayland || { echo 'session-scoping patch not applied'; exit 1; }; \
    fi; \
    test -s /etc/dconf/db/local && test -f /etc/dconf/profile/user && qfile -q /etc/dconf/db/local && qfile -q /etc/portage/repos.conf/horcrux.conf; \
    # GATE: the os-release names this edition (sys-apps/horcrux-release[gnome-light]; the core layer built the Server one)
    grep -qx 'VARIANT_ID=gnome' /usr/lib/os-release; ! grep -iE '^PRETTY_NAME=.*(live|installer)' /usr/lib/os-release; \
    # keyring PAM: one pam_gnome_keyring set (gdm-password), session after elogind; postinst of horcrux-gnome-defaults-1-r2 fixes pambase's duplicate. Fails the build on a stale base.
    /usr/libexec/horcrux-keyring-pam check && qfile -q /usr/libexec/horcrux-keyring-pam /usr/bin/horcrux-keyring-status; \
    # the shipped tree is a git checkout (snapshot of this build's overlay) so `emaint sync -r horcrux` fast-forwards it to the published overlay (sync-depth 1 = fetch + reset)\
    cd /var/db/repos/horcrux; git init -q -b main; git add -A; { git diff --cached --quiet || git -c user.name=horcrux-image -c user.email=image@horcrux.invalid commit -q -m "image snapshot (desktop edition)"; }; \
    git remote get-url origin >/dev/null 2>&1 || git remote add origin "$(sed -n 's/^sync-uri *= *//p' /etc/portage/repos.conf/horcrux.conf)"; git rev-parse --short HEAD; \
    git config branch.main.remote origin; git config branch.main.merge refs/heads/main; git update-ref refs/remotes/origin/main HEAD; git rev-parse --abbrev-ref '@{upstream}' | grep -qx origin/main

COPY project/variants/gnome-light/overlay/ /

COPY common/vdb-prune.py /tmp/vdb-prune.py
COPY project/public-forbidden.txt /tmp/public-forbidden.txt

# the Desktop image marks itself (the installer offers the desktop profile only when this exists); slim down again
# Size: the ISO must stay below the 2 GiB release-asset limit. Firmware for hardware that is not in an x86 laptop/desktop
# (Snapdragon SoCs, server/SmartNICs) is ~900 MB of the uncompressed image; the server image keeps it.
# KEEP_DEV=1 (build.sh passes the environment variable; private machine images such as hogsmeade that compile kernel modules, NVIDIA userland and apps on top of this
# layer and have no 2 GiB limit): headers, static libs and the LLVM tools stay. Without it every later source build dies on a missing header
# (2026-11-10: gui-libs/egl-gbm, `EGL/egl.h: No such file or directory`, libglvnd's headers were stripped here).
ARG KEEP_DEV=
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
    # KEEP the toolchain's own development files (glibc + kernel headers + the libraries kernel-module builds link: elfutils, openssl, zlib, zstd, xz, ncurses;\
    # glib + elogind + openrc because gnome-base/gnome-session-openrc::horcrux is a source package and its updates compile on the installed system):\
    # stripping them left every installed system unable to compile anything (gringotts, 2026-11-03: `C compiler cannot create executables`, missing\
    # libc_nonshared.a/stdio.h), which also breaks the zfs-kmod rebuild on every kernel update. ~100 MB; the rest of the headers/static libs go.\
    if [ -z "$KEEP_DEV" ]; then \
    qlist -e sys-libs/glibc sys-kernel/linux-headers dev-libs/glib sys-auth/elogind sys-apps/openrc dev-libs/elfutils dev-libs/openssl sys-libs/zlib app-arch/zstd app-arch/xz-utils sys-libs/ncurses sys-libs/libxcrypt | grep -E '^/usr/(include|lib64?)/' | sort -u > /tmp/devkeep.lst; \
    test "$(wc -l < /tmp/devkeep.lst)" -gt 500; \
    find /usr/lib /usr/lib64 -xdev -name '*.a' -type f ! -path '/usr/lib/gcc/*' 2>/dev/null | grep -vxFf /tmp/devkeep.lst | xargs -r rm -f; \
    find /usr/include -xdev \( -type f -o -type l \) 2>/dev/null | grep -vxFf /tmp/devkeep.lst | xargs -r rm -f; find /usr/include -xdev -depth -type d -empty -delete 2>/dev/null || true; \
    rm -rf /usr/lib/llvm/*/include /usr/lib/llvm/*/bin /usr/lib/clang 2>/dev/null || true; \
    rm -f /tmp/devkeep.lst; else echo 'KEEP_DEV: development files kept'; test -f /usr/include/EGL/egl.h; fi; \
    # gate: the toolchain still builds a C program that needs libc_nonshared.a and the kernel headers\
    printf '#include <stdio.h>\n#include <linux/types.h>\nint main(void){puts("ok");return 0;}\n' > /tmp/t.c && gcc /tmp/t.c -o /tmp/t && /tmp/t && rm -f /tmp/t /tmp/t.c /tmp/devkeep.lst; \
    echo "desktop" > /etc/live-iso-variant; echo "gnome-light" >> /etc/live-iso-variant; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/cache/binhost /usr/share/doc /usr/share/gtk-doc /var/tmp/portage /var/log/emerge* 2>/dev/null || true; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true; \
    # Portage's records match what was stripped on purpose (the reviewed list in common/vdb-prune.py); anything else missing fails the package-contents gate
    g=core,dev; [ -z "$KEEP_DEV" ] || g=core; python3 /tmp/vdb-prune.py --groups $g && rm -f /tmp/vdb-prune.py
