# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner rule: Steam runs on HOGSMEADE and nowhere else, never in a public image or a lab/test machine). Registers ::steam-overlay
# (Valve's Steam client for Gentoo; it is not in ::gentoo or GURU) through a repos.conf this package owns, plus the abi_x86_32 USE settings the
# 32-bit client needs with the NVIDIA driver. Same plan as games-hogsmeade/hogsmeade-steam-config in ctg-private, packaged in the machine layer
# because HOGSMEADE has no ctg-private checkout.
EAPI=8

DESCRIPTION="HOGSMEADE: ::steam-overlay repos.conf and the 32-bit (abi_x86_32) USE/keyword/licence settings for Steam"
HOMEPAGE="https://github.com/anyc/steam-overlay"
S="${WORKDIR}"
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="dev-vcs/git"

src_install() {
	insinto /etc/portage/repos.conf
	newins - steam-overlay.conf <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-steam-repo)
		[steam-overlay]
		location = /var/db/repos/steam-overlay
		sync-type = git
		sync-uri = https://github.com/anyc/steam-overlay.git
		auto-sync = yes
		priority = 50
	EOF2
	insinto /etc/portage/package.accept_keywords
	newins - steam <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-steam-repo)
		games-util/steam-launcher ~amd64
		games-util/steam-meta ~amd64
		games-util/game-device-udev-rules ~amd64
		sys-libs/libudev-compat ~amd64
	EOF2
	insinto /etc/portage/package.license
	newins - steam <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-steam-repo): Valve's licence is in ::steam-overlay/licenses
		games-util/steam-launcher ValveSteamLicense
		games-util/steam-meta ValveSteamLicense
	EOF2
	insinto /etc/portage/package.use
	newins - steam <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-steam-repo): 32-bit libraries for the 32-bit Steam client and games, NVIDIA proprietary driver, multilib profile
		media-libs/libcanberra alsa
		media-libs/fontconfig abi_x86_32
		sys-libs/libudev-compat abi_x86_32
		x11-libs/libdrm abi_x86_32
		media-libs/libpulse abi_x86_32
		media-libs/vulkan-loader abi_x86_32
		x11-drivers/nvidia-drivers abi_x86_32
		media-libs/vulkan-layers abi_x86_32
		dev-util/spirv-tools abi_x86_32
		dev-util/glslang abi_x86_32
		dev-util/vulkan-utility-libraries abi_x86_32
		dev-libs/wayland abi_x86_32
		x11-libs/libX11 abi_x86_32
		x11-libs/libXrandr abi_x86_32
		x11-libs/libXext abi_x86_32
		x11-libs/libXrender abi_x86_32
		dev-libs/libffi abi_x86_32
		media-libs/libglvnd abi_x86_32
		gui-libs/egl-gbm abi_x86_32
		gui-libs/egl-wayland abi_x86_32
		gui-libs/egl-wayland2 abi_x86_32
		gui-libs/egl-x11 abi_x86_32
		media-libs/mesa abi_x86_32
		x11-libs/libxcb abi_x86_32
		dev-libs/expat abi_x86_32
		llvm-core/llvm abi_x86_32
		media-libs/libdisplay-info abi_x86_32
		x11-libs/libxshmfence abi_x86_32
		x11-libs/libXxf86vm abi_x86_32
		x11-libs/libXfixes abi_x86_32
		x11-libs/xcb-util-keysyms abi_x86_32
		media-libs/freetype harfbuzz abi_x86_32
		media-libs/libsndfile abi_x86_32
		net-libs/libasyncns abi_x86_32
		sys-apps/dbus abi_x86_32
		dev-libs/glib abi_x86_32
		media-libs/flac abi_x86_32
		media-libs/libogg abi_x86_32
		media-libs/libvorbis abi_x86_32
		media-libs/opus abi_x86_32
		media-sound/lame abi_x86_32
		media-sound/mpg123-base abi_x86_32
		sys-apps/util-linux abi_x86_32
		virtual/libintl abi_x86_32
		virtual/libudev abi_x86_32
		sys-apps/systemd-utils abi_x86_32
		virtual/zlib abi_x86_32
		sys-libs/zlib abi_x86_32
		virtual/opengl abi_x86_32
		x11-base/xwayland libei
		dev-libs/libpcre2 abi_x86_32
		x11-libs/libXau abi_x86_32
		x11-libs/libXdmcp abi_x86_32
		app-arch/bzip2 abi_x86_32
		media-libs/libpng abi_x86_32
		media-libs/harfbuzz abi_x86_32
		x11-libs/cairo abi_x86_32
		media-gfx/graphite2 abi_x86_32
		virtual/libiconv abi_x86_32
		x11-libs/pixman abi_x86_32
		dev-util/sysprof-capture abi_x86_32
		app-arch/xz-utils abi_x86_32
		dev-libs/elfutils abi_x86_32
		virtual/libelf abi_x86_32
		x11-libs/libpciaccess abi_x86_32
		dev-libs/icu abi_x86_32
		dev-libs/libxml2 abi_x86_32
		# the host drives an NVIDIA GPU (nvidia-drivers), VIDEO_CARDS in the profile lists only the free drivers
		games-util/steam-launcher video_cards_nvidia
	EOF2
}

pkg_postinst() {
	elog "::steam-overlay is registered. Sync it once (emaint sync -r steam-overlay), then app-misc/hogsmeade-steam can install the client."
}
