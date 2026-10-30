# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="t2linux audio profile sets (speakers/headphones/mics) for PipeWire/PulseAudio on Apple T2 Macs"
HOMEPAGE="https://github.com/kekrby/t2-better-audio"
S="${WORKDIR}"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="media-video/pipewire"

src_install() {
	local d
	for d in usr/share/alsa-card-profile/mixer usr/share/pulseaudio/alsa-mixer; do
		insinto /${d}/profile-sets
		doins "${FILESDIR}"/profile-sets/*
		insinto /${d}/paths
		doins "${FILESDIR}"/paths/*
	done
	insinto /usr/lib/udev/rules.d
	doins "${FILESDIR}"/91-audio-custom.rules
	dodoc "${FILESDIR}"/LICENSE.t2-better-audio
}
