# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's personal app, never in a public image, the Horcrux overlay or a lab machine): OBS Studio with NVENC + PipeWire capture and the
# virtual camera. This package is the layer's "set" (RDEPEND = the apps, so @world keeps them) and owns the machine config they need.
EAPI=8

DESCRIPTION="HOGSMEADE: OBS Studio (NVENC, PipeWire, virtual camera) and its keyword/USE/module configuration"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

# the USE requirements mirror the package.use file below (the image build resolves them from the variant's own portage/ files first)
RDEPEND="
	media-video/obs-studio[nvenc,screencast,v4l]
	media-video/ffmpeg[opus,x264,nvenc]
	media-plugins/obs-pipewire-audio-capture
	media-video/v4l2loopback
"

src_install() {
	insinto /etc/portage/package.accept_keywords
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		media-video/obs-studio ~amd64
		media-plugins/obs-pipewire-audio-capture ~amd64
	EOF2
	insinto /etc/portage/package.use
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		media-video/ffmpeg opus x264 nvenc
		media-video/obs-studio nvenc screencast v4l
		media-video/v4l2loopback dist-kernel
	EOF2

	# virtual camera: one fixed device /dev/video10, loaded at boot by OpenRC's `modules` service (it reads /etc/modules-load.d, no conf.d/modules edit)
	insinto /etc/modprobe.d
	newins - v4l2loopback.conf <<-'EOF2'
		options v4l2loopback devices=1 video_nr=10 card_label="OBS Virtual Camera" exclusive_caps=1
	EOF2
	insinto /etc/modules-load.d
	newins - v4l2loopback.conf <<-'EOF2'
		v4l2loopback
	EOF2
}
