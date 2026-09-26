# Horcrux — Gentoo-native container build
#
# This builds Horcrux the same way it's meant to be installed: through
# Portage, against the project's own ebuild and overlay in gentoo/, with
# real USE flags — not a bare `cargo build`. That's the whole point of
# Gentoo: users choose which features actually get compiled in.
#
# Override USE flags at build time, e.g.:
#   docker build --build-arg HORCRUX_USE="qemu cli webui -docker -podman" .

FROM gentoo/stage3:amd64-systemd AS builder

ARG HORCRUX_USE="qemu cli monitoring systemd webui"

# Portage's stage3 default FEATURES enable ipc-sandbox/network-sandbox/
# pid-sandbox, which require unprivileged user-namespace unshare() calls
# (CAP_SYS_ADMIN-equivalent). CI runners (Forgejo/GitHub Actions Docker-in-
# Docker) generally don't grant this, so emerge fails with
# "Unable to unshare: EPERM" on every build step. Keep file-level
# sandbox/usersandbox (no namespaces needed) but drop the namespace-based
# ones, matching how most Gentoo Docker CI setups build in containers.
RUN echo 'FEATURES="${FEATURES} -ipc-sandbox -network-sandbox -pid-sandbox"' \
    >> /etc/portage/make.conf

# Sync the Gentoo package tree (uses the webrsync snapshot method, no full
# rsync mirror needed inside a container build)
RUN emerge-webrsync

# Register this project's own overlay as a local Portage repo
RUN mkdir -p /var/db/repos/horcrux-overlay/{app-emulation,metadata,profiles} && \
    echo 'masters = gentoo' > /var/db/repos/horcrux-overlay/metadata/layout.conf && \
    echo 'thin-manifests = true' >> /var/db/repos/horcrux-overlay/metadata/layout.conf && \
    echo 'horcrux-overlay' > /var/db/repos/horcrux-overlay/profiles/repo_name && \
    mkdir -p /etc/portage/repos.conf
COPY gentoo/app-emulation /var/db/repos/horcrux-overlay/app-emulation
RUN printf '[horcrux-overlay]\nlocation = /var/db/repos/horcrux-overlay\npriority = 50\n' \
    > /etc/portage/repos.conf/horcrux-overlay.conf

# Bring in the project's real Portage config: USE flags, keywords, package set
COPY gentoo/package.use/horcrux /etc/portage/package.use/horcrux
COPY gentoo/package.accept_keywords/horcrux /etc/portage/package.accept_keywords/horcrux
RUN echo "app-emulation/horcrux ${HORCRUX_USE}" > /etc/portage/package.use/horcrux-docker-build

# The ebuild pulls source via the project's release tarball/vendored crates;
# for a from-source container build we vendor the working tree directly
# instead of fetching a tagged release.
WORKDIR /var/db/repos/horcrux-overlay/app-emulation/horcrux
COPY . /usr/src/horcrux
RUN cd /usr/src/horcrux && cargo vendor /var/cache/distfiles/horcrux-vendor 2>&1 | tail -5 || true

# git-r3 would otherwise re-clone from the remote (EGIT_REPO_URI) even though
# we just vendored the local checkout above, silently ignoring uncommitted
# local changes. EGIT_OVERRIDE_REPO_<PN> is git-r3's documented mechanism to
# point it at a local path instead - this Docker build now genuinely
# reflects what's in this checkout, not whatever's on the remote's HEAD/tag.
ENV EGIT_OVERRIDE_REPO_HORCRUX=/usr/src/horcrux

# Generate a real Manifest for the ebuild (thin-manifests only needs
# Manifest.gz-style DIST entries when SRC_URI points at real distfiles;
# for this from-source dev build there's nothing to fetch, so an empty/
# generated Manifest satisfies Portage's manifest-verification check)
RUN ebuild /var/db/repos/horcrux-overlay/app-emulation/horcrux/horcrux-0.1.0.ebuild manifest --force

# Build and install via emerge, exactly like a real Gentoo host would.
# --quiet-build=y suppresses Portage's per-package compiler noise (only
# showing merge progress + real errors) so the combined build log stays
# under CI's log-size cap across ~77 packages; failures still surface
# their full build log via FEATURES=buildlog + the emerge failure summary.
RUN emerge --verbose --quiet-build=y --autounmask-write app-emulation/horcrux && \
    etc-update --automode -5 || true
RUN emerge --verbose --quiet-build=y app-emulation/horcrux

# --- Runtime stage --------------------------------------------------------
# Still Gentoo — a slim stage3 with only the installed package and its
# runtime deps carried over, so USE-flag-gated components remain accurate.
FROM gentoo/stage3:amd64-systemd

COPY --from=builder /usr/bin/horcrux-api /usr/local/bin/horcrux-api
COPY --from=builder /etc/horcrux /etc/horcrux
COPY --from=builder /var/lib/horcrux /var/lib/horcrux

RUN useradd -m -u 1000 -s /bin/bash horcrux 2>/dev/null || \
    (groupadd horcrux && useradd -m -u 1000 -g horcrux -s /bin/bash horcrux) && \
    chown -R horcrux:horcrux /var/lib/horcrux /etc/horcrux

USER horcrux
EXPOSE 8006
EXPOSE 5900-5999
VOLUME ["/var/lib/horcrux", "/var/log/horcrux"]

HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:8006/api/health || exit 1

WORKDIR /var/lib/horcrux
CMD ["/usr/local/bin/horcrux-api"]
