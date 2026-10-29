# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Apple T2 Mac Broadcom Wi-Fi/Bluetooth firmware (extracted from the owner's own macOS by t2linux firmware.sh)"
HOMEPAGE="https://wiki.t2linux.org/guides/wifi-bluetooth/"
# PRIVATE: Apple's proprietary blobs are not redistributable. This ebuild lives only in the machine-local overlay
# (/var/db/repos/local); the tarball is placed by the owner in DISTDIR (/var/cache/distfiles) by hand.
SRC_URI="firmware.tar"
S="${WORKDIR}"

LICENSE="all-rights-reserved"
RESTRICT="fetch mirror bindist strip"
SLOT="0"
KEYWORDS="-* ~amd64"

pkg_nofetch() {
	einfo "Place the t2linux firmware.tar (from macOS firmware.sh) at ${DISTDIR}/firmware.tar,"
	einfo "then run: ebuild ${EBUILD} manifest && emerge sys-firmware/apple-t2-wifi-firmware"
}

src_unpack() {
	mkdir fw && cd fw || die
	tar -xf "${DISTDIR}/firmware.tar" --no-same-owner --no-same-permissions || die
}

src_install() {
	insinto /lib/firmware/brcm
	local f n=0
	for f in fw/*; do
		case ${f##*/} in
			brcmfmac*apple*|brcmbt*apple*) doins "${f}"; (( n++ )) ;;
		esac
	done
	(( n > 0 )) || die "no Apple brcm firmware files in the tarball"
}
