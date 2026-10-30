# Copyright 2026 Horcrux
# Distributed under the terms of the GNU General Public License v2
EAPI=8

inherit desktop unpacker xdg

DESCRIPTION="Brave web browser (upstream stable release, repackaged from the official .deb)"
HOMEPAGE="https://brave.com"
SRC_URI="https://github.com/brave/brave-browser/releases/download/v${PV}/brave-browser_${PV}_amd64.deb"
S="${WORKDIR}"

LICENSE="MPL-2.0 BSD"
SLOT="0"
KEYWORDS="-* amd64"
RESTRICT="mirror strip"

RDEPEND="
	>=dev-libs/nss-3.26
	dev-libs/glib:2
	dev-libs/expat
	media-libs/alsa-lib
	media-libs/mesa[gbm(+)]
	net-print/cups
	sys-apps/dbus
	x11-libs/cairo
	x11-libs/gtk+:3
	x11-libs/libdrm
	x11-libs/libxkbcommon
	x11-libs/pango
	x11-misc/xdg-utils
	app-accessibility/at-spi2-core:2
"
QA_PREBUILT="opt/brave.com/*"

src_unpack() {
	unpack_deb ${A}
}

src_install() {
	insinto /opt
	doins -r opt/brave.com
	local f
	for f in brave brave-browser chrome_crashpad_handler chrome-management-service default-app-block; do
		[[ -e opt/brave.com/brave/${f} ]] && fperms 0755 /opt/brave.com/brave/${f}
	done
	fperms 4755 /opt/brave.com/brave/chrome-sandbox
	dosym ../../opt/brave.com/brave/brave-browser /usr/bin/brave-browser
	dosym brave-browser /usr/bin/brave
	domenu usr/share/applications/brave-browser.desktop
	local s
	for s in 16 24 32 48 64 128 256; do
		newicon -s ${s} opt/brave.com/brave/product_logo_${s}.png brave-browser.png
	done
}
