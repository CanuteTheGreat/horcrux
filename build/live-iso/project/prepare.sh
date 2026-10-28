#!/bin/sh
# run by build.sh in build/live-iso before `docker build` of the rootfs
set -e
install -D -m 0755 ../installer/horcrux-installer.sh project/overlay/usr/bin/horcrux-installer
