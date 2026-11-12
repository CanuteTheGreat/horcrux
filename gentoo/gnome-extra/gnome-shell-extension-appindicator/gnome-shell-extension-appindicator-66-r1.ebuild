# Copyright 1999-2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

# Horcrux overlay copy of ::gentoo's -66 (generic Horcrux GNOME images carry only this overlay + ::gentoo) with a fix ::gentoo/upstream lack (drop when upstream ships it): Chromium/Electron 43+
# StatusNotifierItems (Slack, Signal, Docker Desktop...) answer Properties.Get/GetAll only when addressed to their
# well-known name org.freedesktop.StatusNotifierItem-<pid>-N and fail with "error occurred in Get" on the unique
# bus name. The patch (a) stops GDBusProxy doing its own initial GetAll on the unique name, (b) introspects the
# well-known name and (c) registers items found by the startup bus scan under their well-known name, so the
# item is no longer destroyed (tray icon flashing "..." and vanishing).
inherit gnome2-utils meson

DESCRIPTION="Support legacy, AppIndicators and KStatusNotifierItems in Gnome"
HOMEPAGE="https://github.com/ubuntu/gnome-shell-extension-appindicator"
MY_PV=${PV%%-*}
SRC_URI="https://github.com/ubuntu/gnome-shell-extension-appindicator/archive/v${MY_PV}.tar.gz -> ${PN}-${MY_PV}.tar.gz"
S="${WORKDIR}/${PN}-${MY_PV}"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	app-eselect/eselect-gnome-shell-extensions
	>=gnome-base/gnome-shell-45
"
BDEPEND="
	app-misc/jq
"

PATCHES=( "${FILESDIR}/${PN}-${MY_PV}-chromium-sni.patch" )

src_install() {
	meson_src_install
	rm -v "${ED}"/usr/share/glib-2.0/schemas/gschemas.compiled || die
}

pkg_preinst() {
	gnome2_schemas_savelist
}

pkg_postinst() {
	gnome2_schemas_update
	ebegin "Updating list of installed extensions"
	eselect gnome-shell-extensions update
	eend $?
}

pkg_postrm() {
	gnome2_schemas_update
}
