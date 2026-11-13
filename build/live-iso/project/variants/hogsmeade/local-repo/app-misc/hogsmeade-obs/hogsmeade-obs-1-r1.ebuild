# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's personal app, never in a public image, the Horcrux overlay or a lab machine): OBS Studio with the FULL feature set (browser source/docks, obs-websocket, VLC source, Lua+Python scripting,
# NVENC + QuickSync, SRT/RIST output, fdk AAC, JACK, speex, DeckLink [bundled API headers, the Blackmagic driver is a separate vendor install]),
# PipeWire audio capture, obs-vkcapture game capture and the virtual camera. This package is the layer's "set" (RDEPEND = the apps, so @world keeps them) and owns the machine config they need.
EAPI=8

DESCRIPTION="HOGSMEADE: OBS Studio (full feature set, NVENC/QSV, PipeWire, vkcapture, virtual camera) and its keyword/USE/module configuration"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

# the USE requirements mirror the package.use file below (the image build resolves them from the variant's own portage/ files first)
RDEPEND="
	media-video/obs-studio[alsa,browser,decklink,fdk,jack,lua,mpegts,nvenc,pulseaudio,python,qsv,screencast,speex,truetype,v4l,vlc,wayland,websocket]
	media-video/ffmpeg[opus,x264,x265,nvenc,qsv,vaapi,svt-av1,vpx,fdk,srt,rist]
	media-plugins/obs-pipewire-audio-capture
	media-plugins/obs-vkcapture[X,wayland]
	media-video/v4l2loopback
"

src_install() {
	insinto /etc/portage/package.accept_keywords
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		media-video/obs-studio ~amd64
		media-plugins/obs-pipewire-audio-capture ~amd64
		media-plugins/obs-vkcapture ~amd64
	EOF2
	# libfdk-aac (AAC encoder): ffmpeg[fdk] is all-rights-reserved + RESTRICT=bindist with USE=gpl, so it is built from source on the machine, never from a binhost
	# obs-studio[vlc] (VLC video source) builds against libvlc 3 only (<vlc-4.0); without the mask Portage downgrades from the 4.0 pre-release snapshot on its own
	insinto /etc/portage/package.mask
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		>=media-video/vlc-4.0
	EOF2
	insinto /etc/portage/package.license
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		media-video/ffmpeg all-rights-reserved
		media-libs/fdk-aac FraunhoferFDK
	EOF2
	insinto /etc/portage/package.use
	newins - obs-studio <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-obs)
		media-video/ffmpeg opus x264 x265 nvenc qsv vaapi svt-av1 vpx fdk srt rist
		media-video/obs-studio alsa browser decklink fdk jack lua mpegts nvenc pulseaudio python qsv screencast speex truetype v4l vlc wayland websocket
		media-plugins/obs-vkcapture X wayland
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
