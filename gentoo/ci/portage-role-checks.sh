#!/bin/bash
# Real-Portage checks of the 2026-11 role/manager packages, in the image builder's rootfs container (never on crystalcave; run on rw-man-01).
# usage (on rw-man-01, repo rsynced to /tmp/lab-unblock):  docker run --rm -v /tmp/lab-unblock:/src:ro -v $PWD/gentoo/ci/portage-role-checks.sh:/c.sh --memory=4g <rootfs image> bash /c.sh
# 1) sys-apps/horcrux-role-containers resolves with ONLY the files repo-config ships (keywords + USE);  2) app-emulation/horcrux-bin resolves (qemu[spice]);
# 3) the horcrux-bin ebuild installs a tarball in the exact layout Dockerfile.app produces and fails loudly without one.
emerge-webrsync -q >/dev/null 2>&1
mkdir -p /var/db/repos/horcrux/metadata /var/db/repos/horcrux/profiles /etc/portage/package.accept_keywords /etc/portage/package.use /etc/portage/repos.conf
cp -a /src/gentoo/app-emulation /src/gentoo/sys-apps /var/db/repos/horcrux/
printf "masters = gentoo\nthin-manifests = true\n" > /var/db/repos/horcrux/metadata/layout.conf; echo horcrux > /var/db/repos/horcrux/profiles/repo_name
printf "[horcrux]\nlocation = /var/db/repos/horcrux\nmasters = gentoo\n" > /etc/portage/repos.conf/horcrux.conf
R=/src/gentoo/sys-apps/horcrux-repo-config/files
cp $R/horcrux-roles.keywords /etc/portage/package.accept_keywords/horcrux-roles
cp $R/horcrux-roles.use /etc/portage/package.use/horcrux-roles
cp /src/build/live-iso/project/portage/package.use/horcrux-virt /etc/portage/package.use/
touch /etc/portage/package.use/zz-found /etc/portage/package.accept_keywords/zz-found
for i in $(seq 1 30); do
  emerge -pq --usepkg --update --newuse sys-apps/horcrux-role-containers >/tmp/o 2>&1
  new=$(grep -oE '^- [a-z0-9-]+/[A-Za-z0-9+_.-]+::gentoo \(masked by: ~amd64 keyword\)' /tmp/o | sed -E 's/^- //; s/::gentoo.*//; s/-[0-9][0-9A-Za-z._+]*(-r[0-9]+)?$//' | sort -u)
  chg=$(grep -E '^- [a-z0-9-]+/.*\(Change USE: ' /tmp/o | sed -E 's/^- //; s/-[0-9][0-9A-Za-z._+]*(-r[0-9]+)?::gentoo \(Change USE: \+?/ /; s/\)$//')
  [ -z "$new" ] && [ -z "$chg" ] && break
  for n in $new; do echo "$n ~amd64 ~arm64" >> /etc/portage/package.accept_keywords/zz-found; done
  [ -n "$chg" ] && echo "$chg" >> /etc/portage/package.use/zz-found
done
echo "--- iterations $i; last output"; tail -12 /tmp/o | cut -c1-250
echo "--- USE found"; cat /etc/portage/package.use/zz-found
echo "--- keywords found"; cat /etc/portage/package.accept_keywords/zz-found
if [ -s /etc/portage/package.use/zz-found ] || [ -s /etc/portage/package.accept_keywords/zz-found ]; then echo "FAIL: role package needs more keywords/USE than horcrux-repo-config ships"; FAIL=1; fi
grep -q "horcrux-role-containers-1" /tmp/o || { echo "FAIL: role package does not resolve"; FAIL=1; }
emerge -pvq --usepkg app-emulation/horcrux-bin 2>&1 | grep -E "horcrux-bin|spice" | cut -c1-200 || FAIL=1
# fake manager tarball in the exact layout Dockerfile.app produces
W=/tmp/fake; rm -rf $W; mkdir -p $W/usr/bin $W/usr/share/horcrux/ui $W/etc/init.d $W/etc/conf.d
printf '#!/bin/sh\necho api\n' > $W/usr/bin/horcrux-api; printf '#!/bin/sh\necho cli\n' > $W/usr/bin/horcrux; chmod +x $W/usr/bin/*
echo ui > $W/usr/share/horcrux/ui/index.html; printf '#!/sbin/openrc-run\n' > $W/etc/init.d/horcrux-api; cp $W/etc/init.d/horcrux-api $W/etc/init.d/horcrux-guests; echo x > $W/etc/conf.d/horcrux-api
(cd $W && tar --zstd -cf /tmp/horcrux-manager-test.tar.zst --owner=0 --group=0 usr/bin/horcrux usr/bin/horcrux-api usr/share/horcrux/ui etc/init.d/horcrux-api etc/init.d/horcrux-guests etc/conf.d/horcrux-api)
cd /var/db/repos/horcrux/app-emulation/horcrux-bin
export HORCRUX_MANAGER_TARBALL=/tmp/horcrux-manager-test.tar.zst
ebuild horcrux-bin-0.1.0.ebuild clean unpack install 2>&1 | tail -15
echo "=== image tree"; find /var/tmp/portage/app-emulation/horcrux-bin-0.1.0/image -type f -o -type l | sed 's#.*/image##' | sort
echo "=== negative: no tarball"; unset HORCRUX_MANAGER_TARBALL; HORCRUX_MANAGER_URL=http://127.0.0.1:1/x ebuild horcrux-bin-0.1.0.ebuild clean unpack 2>&1 | tail -4
[ -z "${FAIL:-}" ] && echo "ALL OK" || { echo "FAILED"; exit 1; }
