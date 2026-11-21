# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Apple T2 Mac Wi-Fi/Bluetooth firmware taken from this Mac itself (installed so the files are package-owned)"
HOMEPAGE="https://wiki.t2linux.org/guides/wifi-bluetooth/"
# No blob is distributed with this package. The Horcrux installer reads the firmware from the Mac it installs on (macOS volume or
# t2linux's firmware-raw.tar.gz on the Mac's EFI partition, converted to Linux names) or from the owner's machine profile bundle, and
# puts it at APPLE_T2_FIRMWARE_TAR (default below); `horcrux-t2-firmware <firmware.tar|dir>` does the same on an installed system.
# No SRC_URI on purpose: the tarball differs per Mac, so it cannot have a Manifest checksum.
: "${APPLE_T2_FIRMWARE_TAR:=/var/cache/distfiles/apple-t2-firmware-local.tar}"
S="${WORKDIR}"

LICENSE="all-rights-reserved"
SLOT="0"
KEYWORDS="-* ~amd64"
RESTRICT="mirror bindist strip"

pkg_pretend() {
	if [[ ! -r ${APPLE_T2_FIRMWARE_TAR} ]]; then
		eerror "${APPLE_T2_FIRMWARE_TAR} is missing. This package installs the Wi-Fi/Bluetooth firmware of THIS Mac:"
		eerror "the Horcrux installer captures it automatically; on an installed system run horcrux-t2-firmware <firmware.tar|dir>."
		die "no firmware tarball"
	fi
}

src_unpack() {
	mkdir fw || die
	tar -C fw -xf "${APPLE_T2_FIRMWARE_TAR}" --no-same-owner --no-same-permissions || die
}

src_install() {
	insinto /lib/firmware/brcm
	local f n=0
	for f in fw/*; do
		case ${f##*/} in
			brcmfmac*apple*|brcmbt*apple*) doins "${f}"; (( n++ )) ;;
		esac
	done
	(( n > 0 )) || die "no Apple brcm firmware files in ${PN}.tar"
}
