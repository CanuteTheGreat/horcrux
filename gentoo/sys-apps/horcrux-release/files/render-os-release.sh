#!/bin/bash
# Prints the Horcrux /usr/lib/os-release (os-release(5)) on stdout.
#   render-os-release.sh <PV> <edition: server|gnome-light> [machine: hogsmeade|hogshead|gringotts|surface|test-machine]
# PV is the package version, 0.1.0 or 0.1.0_p<YYYYMMDDHHMM> (the stamp = the build id; it moves with every horcrux-bin bump).
# This is the INSTALLED system's file: it never says live/installer (the live session marks itself at boot, see live-os-release).
set -eu
pv=${1:?PV}; edition=${2:?edition}; machine=${3:-}
ver=${pv%%_*}; build=""
case "$pv" in *_p[0-9]*) build=${pv##*_p}; build=${build%%-*} ;; esac
short=$(echo "$ver" | cut -d. -f1,2)
case "$edition" in
	server) elabel="Server" ;;
	gnome-light) elabel="GNOME Light" ;;
	*) echo "unknown edition: $edition" >&2; exit 2 ;;
esac
vid=$edition; vlabel=$elabel; detail=$elabel
case "$machine" in
	"") ;;
	hogsmeade|hogshead|gringotts|surface) mlabel=$(echo "${machine:0:1}" | tr a-z A-Z)${machine:1} ;;
	test-machine) mlabel="Test Machine" ;;
	*) echo "unknown machine: $machine" >&2; exit 2 ;;
esac
if [ -n "$machine" ]; then vid=$machine; vlabel=$mlabel; detail="$mlabel, $elabel"; fi
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
