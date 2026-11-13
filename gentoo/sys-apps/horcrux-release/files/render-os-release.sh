#!/bin/bash
# Prints the Horcrux /usr/lib/os-release (os-release(5)) on stdout.
#   render-os-release.sh <PV> <edition: server|gnome-light>
# VARIANT/VARIANT_ID are the edition only; machine/host names never appear.
# PV is the package version, 0.1.0 or 0.1.0_p<YYYYMMDDHHMM> (the stamp = the build id; it moves with every horcrux-bin bump).
# This is the INSTALLED system's file: it never says live/installer (the live session marks itself at boot, see live-os-release).
set -eu
pv=${1:?PV}; edition=${2:?edition}
ver=${pv%%_*}; build=""
case "$pv" in *_p[0-9]*) build=${pv##*_p}; build=${build%%-*} ;; esac
short=$(echo "$ver" | cut -d. -f1,2)
case "$edition" in
	server) elabel="Server"; vid=server ;;
	gnome-light) elabel="GNOME"; vid=gnome ;;  # "light" is an internal build-variant name, not user-facing
	*) echo "unknown edition: $edition" >&2; exit 2 ;;
esac
vlabel=$elabel; detail=$elabel
cat <<OSR
NAME="Horcrux"
ID=horcrux
ID_LIKE=gentoo
PRETTY_NAME="Horcrux $short ($detail)"
VERSION="$ver"
VERSION_ID=$ver
${build:+BUILD_ID=$build
}VARIANT="$vlabel"
VARIANT_ID=$vid
ANSI_COLOR="0;35"
LOGO=horcrux-logo
HOME_URL="https://horcrux.errant.solutions/"
DOCUMENTATION_URL="https://horcrux.errant.solutions/docs/"
SUPPORT_URL="https://github.com/CanuteTheGreat/horcrux/issues"
BUG_REPORT_URL="https://github.com/CanuteTheGreat/horcrux/issues"
OSR
