# Copyright 2026 Horcrux Contributors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3 linux-info systemd

DESCRIPTION="Horcrux guest tools: in-guest capture/encode, input, clipboard and layout for Horcrux Console"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
EGIT_REPO_URI="https://github.com/CanuteTheGreat/horcrux.git"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""
IUSE="vaapi X wayland"

# FFmpeg does the capture (kmsgrab/x11grab) and the encode (libx264,
# VA-API, NVENC); clipboard and layout use the session's own tools.
RDEPEND="
	media-video/ffmpeg[X?,drm,x264,opus,pulseaudio,vaapi?]
	X? ( x11-misc/xclip x11-apps/xrandr )
	wayland? ( gui-apps/wl-clipboard )
"
BDEPEND="virtual/rust"

CONFIG_CHECK="~INPUT_UINPUT ~VIRTIO_CONSOLE"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_compile() {
	cargo_src_compile -p horcrux-guest-tools
}

src_install() {
	dobin "$(cargo_target_dir)"/horcrux-guest-agent
	newinitd packaging/linux/horcrux-guest-agent.initd horcrux-guest-agent
	systemd_dounit packaging/linux/horcrux-guest-agent.service
	insinto /etc/horcrux
	newins packaging/linux/guest-tools.conf guest-tools.conf
}

pkg_postinst() {
	elog "Enable the agent so Horcrux Console can use it:"
	elog "  OpenRC:  rc-update add horcrux-guest-agent default && rc-service horcrux-guest-agent start"
	elog "  systemd: systemctl enable --now horcrux-guest-agent"
}
