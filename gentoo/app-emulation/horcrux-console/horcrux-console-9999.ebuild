# Copyright 2026 Horcrux Contributors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3

DESCRIPTION="Horcrux Console desktop client: VM consoles in a native window (raw mouse, gamepads, high refresh)"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
EGIT_REPO_URI="https://github.com/CanuteTheGreat/horcrux.git"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""
IUSE="X wayland"
REQUIRED_USE="|| ( X wayland )"

# Decoding uses the system FFmpeg libraries (libavcodec's native H.264
# decoder) and the window system libraries, all loaded at run time; a client
# app, so there is no init script or unit.
RDEPEND="
	media-video/ffmpeg
	x11-libs/libxkbcommon[X?,wayland?]
	X? ( x11-libs/libX11 x11-libs/libXcursor x11-libs/libXi x11-libs/libXrandr )
	wayland? ( dev-libs/wayland )
"
BDEPEND="virtual/rust"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile -p horcrux-desktop
}

src_test() {
	cargo_src_test -p horcrux-desktop -p horcrux-media
}

src_install() {
	dobin "$(cargo_target_dir)"/horcrux-console
	insinto /usr/share/horcrux-console
	newins packaging/linux/horcrux-console.toml console.toml.example
	einstalldocs
}

pkg_postinst() {
	elog "Connect with:  horcrux-console --node https://NODE:8006 --vm VMID"
	elog "Defaults live in \${XDG_CONFIG_HOME:-~/.config}/horcrux/console.toml;"
	elog "see /usr/share/horcrux-console/console.toml.example (horcrux-console --print-defaults)."
	elog "Gamepads are read from /dev/input: add the user to the 'input' group."
}
