# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit gnome2-utils

DESCRIPTION="Horcrux Surface tablet desktop: XFCE + LightDM, touch/pen, on-screen keyboard, power policy, the owner's apps"
HOMEPAGE="https://git.canutethegreat.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"

# Everything the Surface desktop needs, as dependencies of ONE package: `emerge -uDN @world` keeps all of it current.
# onboard + iio-sensor-proxy come from GURU, brave-bin from the owner's canutethegreat-overlay,
# rustinion-bin from the Horcrux overlay.
RDEPEND="
	sys-kernel/horcrux-surface-config
	<sys-kernel/gentoo-kernel-6.19
	sys-kernel/horcrux-unlock
	sys-apps/horcrux-tools
	sys-apps/horcrux-live-data
	xfce-base/xfce4-meta
	xfce-base/xfce4-power-manager
	xfce-extra/xfce4-screensaver
	xfce-extra/xfce4-pulseaudio-plugin
	xfce-extra/xfce4-notifyd
	xfce-extra/xfce4-taskmanager
	x11-terms/xfce4-terminal
	x11-misc/lightdm
	x11-misc/lightdm-gtk-greeter
	gnome-extra/nm-applet
	net-misc/networkmanager
	media-video/pipewire
	media-video/wireplumber
	media-sound/pavucontrol
	app-accessibility/onboard
	gnome-extra/iio-sensor-proxy
	x11-drivers/xf86-input-libinput
	x11-drivers/xf86-input-wacom
	x11-apps/xinput
	x11-apps/xprop
	app-portage/portage-utils
	x11-apps/xrandr
	sys-power/upower
	sys-auth/elogind
	sys-apps/xdg-desktop-portal-gtk
	www-client/firefox-bin
	www-client/brave-bin
	net-misc/nextcloud-client
	kde-frameworks/extra-cmake-modules
	net-vpn/tailscale
	app-admin/rustinion-bin
	sys-firmware/intel-microcode
	sys-firmware/sof-firmware
	sys-kernel/linux-firmware
	sys-process/cronie
"

src_install() {
	cp -a "${FILESDIR}/root/." "${ED}/" || die
	local f
	for f in "${ED}"/usr/bin/* "${ED}"/etc/local.d/*.start; do fperms 0755 "${f#"${ED}"}"; done
}

pkg_postinst() {
	gnome2_schemas_update
}

pkg_postrm() {
	gnome2_schemas_update
}
