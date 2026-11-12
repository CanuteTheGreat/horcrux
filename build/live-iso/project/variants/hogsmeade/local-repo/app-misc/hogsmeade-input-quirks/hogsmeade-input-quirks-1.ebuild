# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (this one laptop model: Alienware 18 Area-51 AA18250; never in a public image, the Horcrux overlay or another machine).
# The internal keyboard of this laptop is a USB device ("DELL Technologies Keyboard", usb:0d62:6abc), so libinput treats it as an external
# keyboard and never pairs it with the i2c touchpad (06CB:CFF2): disable-while-typing does nothing. This marks it as internal.
# The quirk is matched on the DMI modalias of this model too, so it can not apply to any other machine (even one with the same USB keyboard).
# libinput reads /usr/share/libinput/*.quirks at startup; the running compositor picks it up at the next login.
# Upstream: the same quirk is offered to libinput (~/.hermes/state/upstream/libinput-alienware-area51.md); drop this package once released.
EAPI=8

DESCRIPTION="HOGSMEADE: libinput quirk marking the internal USB keyboard as internal (fixes touchpad disable-while-typing)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="dev-libs/libinput"

src_install() {
	insinto /usr/share/libinput
	newins - 50-hogsmeade-alienware.quirks <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-input-quirks): Alienware 18 Area-51 AA18250 only.
		[Alienware 18 Area-51 AA18250 internal USB keyboard]
		MatchUdevType=keyboard
		MatchBus=usb
		MatchVendor=0x0D62
		MatchProduct=0x6ABC
		MatchDMIModalias=dmi:*svnAlienware:pnAlienware18Area-51AA18250:*
		AttrKeyboardIntegration=internal
	EOF2
}
