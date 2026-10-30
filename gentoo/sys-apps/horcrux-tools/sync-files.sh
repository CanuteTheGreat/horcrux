#!/bin/sh
# Refresh files/ from the Horcrux live-installer sources (build/live-iso). Run from this directory, then bump the ebuild revision.
set -e
B=${1:-../../../build/live-iso}
rm -rf files; mkdir -p files/usr/bin files/usr/libexec files/usr/lib files/usr/share/horcrux/desktop
cp -a $B/common/overlay/usr/lib/live-installer files/usr/lib/
find files -name __pycache__ -prune -exec rm -rf {} +
for f in live-installer console-menu live-persist live-rescue horcrux-be horcrux-t2-firmware; do cp -a $B/common/overlay/usr/bin/$f files/usr/bin/; done
cp -a $B/common/overlay/usr/libexec/* files/usr/libexec/
cp -a $B/project/variants/desktop/overlay/usr/share/horcrux/desktop/* files/usr/share/horcrux/desktop/
# horcrux-orphans (files/usr/bin/horcrux-orphans) is maintained by hand in this package
