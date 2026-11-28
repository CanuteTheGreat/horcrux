#!/bin/bash
# Regenerates fixtures/zfs-2.4-block-cloning-endian-pool.img.xz: a pool made by a FOREIGN OpenZFS 2.4 build (here Ubuntu 26.04's 2.4.1, the same
# feature set a Proxmox VE 9 host creates) with feature@block_cloning_endian ACTIVE (a `cp --reflink` on 2.4 activates it) and
# dynamic_gang_header/physical_rewrite enabled. OpenZFS 2.3 can import such a pool read-only only; qemu-test.py imports it read-write on the
# installed system. Run in a privileged container on a host whose kernel has the OpenZFS >= 2.4 module loaded:
#   docker run --rm --privileged -v /dev:/dev -v "$PWD":/out ubuntu:26.04 bash /out/make-block-cloning-endian-pool.sh /out
set -e
d=$1; f=$d/bce.img; p=hxbcefix
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null; apt-get install -y -qq zfsutils-linux xz-utils >/dev/null
zfs version | head -1
rm -f "$f" "$f.xz"; truncate -s 64M "$f"
zpool create -f -o cachefile=none -O mountpoint=/hxbcefix -O compression=off $p "$f"
head -c 262144 /dev/urandom > /hxbcefix/a; sync
cp --reflink=always /hxbcefix/a /hxbcefix/b; sha256sum /hxbcefix/a > /hxbcefix/SHA256; sync; zpool sync $p
zpool get -H -o property,value feature@block_cloning,feature@block_cloning_endian,feature@dynamic_gang_header,feature@physical_rewrite $p
zpool get -H -o value feature@block_cloning_endian $p | grep -qx active
zpool export $p
xz -T2 -9 "$f"; mv "$f.xz" "$d/zfs-2.4-block-cloning-endian-pool.img.xz"
