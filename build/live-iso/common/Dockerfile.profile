# syntax=docker/dockerfile:1.4
# Hardware-profile layer (HW_PROFILE=surface ...): ON TOP of the finished rootfs image of the product ISO.
# Context = build/live-iso with the profile directory copied to .hw/ by build.sh (gentoo/<profile>/).
#   gentoo/<profile>/packages.txt       extra packages (binpkg-first; a local overlay for what ::gentoo lacks)
#   gentoo/<profile>/portage/           package.use etc.
#   gentoo/<profile>/portage-local/     local overlay (ebuilds)
#   gentoo/<profile>/overlay/           files copied to /
#   gentoo/<profile>/hwprofile.toml     DMI match + installer defaults (-> /usr/share/live-installer/hwprofiles/<id>.toml)
# Own layer: editing a profile never redoes the big product emerge.
ARG BASE
FROM ${BASE}
ARG ARCH=x86_64
ARG HW_PROFILE
COPY .hw/packages.txt /tmp/hw/packages.txt
COPY .hw/portage/ /tmp/hw/portage/
COPY .hw/portage-local/ /tmp/hw/portage-local/
RUN --mount=type=cache,id=binpkgs-${ARCH},target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=distfiles-${ARCH},target=/var/cache/distfiles,sharing=locked \
    set -e; export FEATURES="-ccache"; \
    emerge-webrsync -q >/dev/null 2>&1 || emerge-webrsync -q; \
    mkdir -p /var/db/repos/horcrux-local /etc/portage/repos.conf; cp -r /tmp/hw/portage-local/. /var/db/repos/horcrux-local/; \
    printf '[horcrux-local]\nlocation = /var/db/repos/horcrux-local\nmasters = gentoo\nauto-sync = no\n' > /etc/portage/repos.conf/horcrux-local.conf; \
    cp -r /tmp/hw/portage/. /etc/portage/; \
    pk=$(grep -v '^#' /tmp/hw/packages.txt | tr '\n' ' '); \
    emerge --noreplace $pk sys-fs/cryptsetup; \
    rm -rf /var/db/repos/gentoo/* /var/cache/distfiles/* /var/tmp/portage /var/log/emerge* /tmp/hw 2>/dev/null || true

# config + scripts: cheap layers (no emerge). project/overlay + common/overlay again (same order as Dockerfile.rootfs): the base image may predate the newest installer code / profile.toml
COPY project/overlay/ /
COPY common/overlay/ /
COPY .hw/overlay/ /
COPY .hw/hwprofile.toml /tmp/hwprofile.toml
RUN set -e; mkdir -p /usr/share/live-installer/hwprofiles; mv /tmp/hwprofile.toml "/usr/share/live-installer/hwprofiles/${HW_PROFILE}.toml"; \
    chmod +x /usr/bin/surface-* /usr/bin/horcrux-surface-report /usr/libexec/surface-* /etc/init.d/zram-swap 2>/dev/null || true; \
    echo "${HW_PROFILE}" > /etc/live-iso-hwprofile; \
    find / -xdev -name '*.pyc' -path '*/__pycache__/*' -delete 2>/dev/null || true
