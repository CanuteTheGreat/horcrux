# syntax=docker/dockerfile:1.4
# hogshead live ISO rootfs: Gentoo stage3 + Gentoo's own kernel sources + the t2linux patch set + OpenZFS + live userland.
# Build context = a staged dir made by build.sh (gentoo/t2/**, build/live-iso/common/{arch,portage}).
# Named contexts: t2patches (pinned patch set), priv (PRIVATE: firmware, authorized_keys, tailscale auth key; may be empty).
ARG STAGE3_TAG=amd64-openrc

FROM gentoo/stage3:${STAGE3_TAG} AS base
RUN --mount=type=cache,id=hs-distfiles,target=/var/cache/distfiles,sharing=locked \
    emerge-webrsync -q && getuto >/dev/null 2>&1; true
COPY build/live-iso/common/arch/x86_64.binrepos.conf /etc/portage/binrepos.conf/gentoo.conf
COPY build/live-iso/common/arch/x86_64.make.conf /etc/portage/make.conf.arch
COPY build/live-iso/common/arch/x86_64.accept_keywords /etc/portage/package.accept_keywords/arch
COPY build/live-iso/common/portage/ /etc/portage/
COPY gentoo/t2/live/portage/ /etc/portage/
RUN cat /etc/portage/make.conf.hogshead >> /etc/portage/make.conf && rm /etc/portage/make.conf.hogshead

# ---------------- kernel: gentoo-sources + t2linux patches ----------------
FROM base AS kernel
ARG KERNEL_PKG
ARG KERNEL_SRC_DIR
ARG LOCALVERSION
RUN --mount=type=cache,id=hs-binpkgs,target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=hs-distfiles,target=/var/cache/distfiles,sharing=locked \
    set -e; \
    emerge --oneshot ${KERNEL_PKG} sys-devel/bc dev-libs/elfutils sys-apps/kmod app-alternatives/cpio dev-libs/openssl; \
    # Gentoo's distribution-kernel config is the base (broad hardware coverage). Only the unpack phase is run: it contains the\
    # dist config of the SAME kernel version (installing the -bin package would need Secure Boot signing keys).\
    FEATURES="-sandbox -usersandbox -network-sandbox -pid-sandbox -ipc-sandbox" \
    ebuild $(ls /var/db/repos/gentoo/sys-kernel/gentoo-kernel-bin/gentoo-kernel-bin-*.ebuild | sort -V | tail -1) clean unpack; \
    cp $(find /var/tmp/portage/sys-kernel -path '*dist-bin/.config' | head -1) /dist.config; \
    rm -rf /var/tmp/portage/sys-kernel; \
    ls /usr/src
COPY gentoo/t2/kernel/ /t2/kernel/
RUN --mount=type=bind,from=t2patches,target=/t2patches \
    KERNEL_SRC_DIR=${KERNEL_SRC_DIR} LOCALVERSION=${LOCALVERSION} sh /t2/kernel/build-kernel.sh > /build-kernel.log 2>&1 \
    || { tail -100 /build-kernel.log; exit 1; }; \
    tail -5 /build-kernel.log; ln -sfn ${KERNEL_SRC_DIR} /usr/src/linux

# ---------------- userland ----------------
FROM base AS rootfs
COPY gentoo/t2/live/packages.txt /tmp/packages.txt
RUN --mount=type=cache,id=hs-binpkgs,target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=hs-distfiles,target=/var/cache/distfiles,sharing=locked \
    set -e; emerge --noreplace $(grep -v '^#' /tmp/packages.txt | tr '\n' ' '); ls /usr/bin/nmtui /usr/bin/cryptsetup

# kernel (+ build tree for out-of-tree ZFS), then OpenZFS built against it
COPY --from=kernel /boot/ /boot/
COPY --from=kernel /lib/modules/ /lib/modules/
COPY --from=kernel /usr/src/ /usr/src/
RUN --mount=type=cache,id=hs-binpkgs,target=/var/cache/binpkgs,sharing=locked \
    --mount=type=cache,id=hs-distfiles,target=/var/cache/distfiles,sharing=locked \
    set -e; ls -l /usr/src; emerge --noreplace =sys-fs/zfs-kmod-2.3.9 =sys-fs/zfs-2.3.9; \
    kr=$(cat /boot/.kernelrelease); find /lib/modules/$kr -name 'zfs.ko*' | grep .; depmod $kr

# tailscale (official static build, sha256-verified)
ARG TS_VERSION=1.104.4
RUN set -e; cd /tmp; curl -fsSLO https://pkgs.tailscale.com/stable/tailscale_${TS_VERSION}_amd64.tgz; \
    echo "$(curl -fsSL https://pkgs.tailscale.com/stable/tailscale_${TS_VERSION}_amd64.tgz.sha256)  tailscale_${TS_VERSION}_amd64.tgz" | sha256sum -c -; \
    tar xzf tailscale_${TS_VERSION}_amd64.tgz; install -m0755 tailscale_${TS_VERSION}_amd64/tailscale tailscale_${TS_VERSION}_amd64/tailscaled /usr/bin/; \
    rm -rf /tmp/tailscale_*; tailscale version | head -1

COPY gentoo/t2/live/overlay/ /
COPY gentoo/t2/dracut/ /etc/dracut.conf.d/
# PRIVATE layer: Apple Wi-Fi/BT firmware, the owner's public ssh keys, the one-time Tailscale auth key (all optional)
COPY --from=priv / /tmp/priv/
COPY gentoo/t2/live/setup-rootfs.sh /tmp/setup-rootfs.sh
RUN sh /tmp/setup-rootfs.sh && rm -rf /tmp/priv /tmp/setup-rootfs.sh
