# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the keyword/licence/USE files for app-misc/hogsmeade-apps. A separate package because Portage must see these files BEFORE it can
# resolve the apps (a set package cannot unmask its own dependencies).
EAPI=8

DESCRIPTION="HOGSMEADE: package.accept_keywords/license/use for the owner's apps (app-misc/hogsmeade-apps-config)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

src_install() {
	insinto /etc/portage/package.accept_keywords
	newins - hogsmeade-apps <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-apps-config)
		dev-util/android-studio ~amd64
		net-im/slack ~amd64
		net-im/zoom ~amd64
		games-emulation/dosbox-staging ~amd64
		games-util/heroic-bin ~amd64
		games-action/minecraft-launcher ~amd64
		media-video/makemkv ~amd64
		media-gfx/loupe ~amd64
		net-analyzer/ookla-speedtest ~amd64
		app-crypt/yubioath-flutter-bin ~amd64
		app-admin/aws-session-manager-plugin ~amd64
		app-office/libreoffice ~amd64
		net-irc/halloy ~amd64
		net-im/teams-for-linux ~amd64
		mail-client/proton-mail-bin ~amd64
		net-im/proton-meet-bin ~amd64
		games-util/geforcenow-electron ~amd64
		net-misc/freedownloadmanager ~amd64
		net-fs/proton-drive-cli-bin ~amd64
		sys-auth/proton-authenticator-bin ~amd64
		app-admin/proton-pass-bin ~amd64
		net-analyzer/caido-bin ~amd64
		media-libs/iir1 ~amd64
		net-misc/onedrive ~amd64
		# geesefs (app-misc/hogsmeade-b2-mounts)
		net-fs/geesefs ~amd64
		# NOT a hogsmeade-apps dependency (fetch-restricted: the zip must be in /var/cache/distfiles); emerged by hand, kept in @world
		media-video/davinci-resolve ~amd64
	EOF2
	insinto /etc/portage/package.license
	newins - hogsmeade-apps <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-apps-config): proprietary and -bin apps (Claude Code, VS Code, Android Studio, Slack, Zoom, MakeMKV, Chrome, Edge, ...)
		dev-util/claude-code *
		app-editors/vscode *
		dev-util/android-studio *
		net-im/slack *
		net-im/zoom *
		net-im/signal-desktop-bin *
		net-im/proton-meet-bin *
		media-video/makemkv *
		net-analyzer/ookla-speedtest *
		net-analyzer/caido-bin *
		net-misc/freedownloadmanager *
		app-admin/proton-pass-bin *
		games-action/minecraft-launcher *
		games-util/heroic-bin *
		mail-client/thunderbird-bin *
		www-client/google-chrome *
		www-client/microsoft-edge *
		app-crypt/yubioath-flutter-bin *
		app-admin/aws-session-manager-plugin *
		dev-java/openjdk-bin *
		dev-dotnet/dotnet-sdk-bin *
		media-video/davinci-resolve all-rights-reserved
	EOF2
	insinto /etc/portage/package.use
	newins - hogsmeade-apps <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-apps-config)
		# Claude Code's prebuilt binary needs AVX/AVX2 (the Alienware's CPU has both)
		dev-util/claude-code cpu_flags_x86_avx cpu_flags_x86_avx2
		media-libs/libv4l utils
		net-dns/avahi python
		sys-libs/zlib minizip
		# OneDrive client: the binhost binpkg is built with libnotify (desktop notifications); --binpkg-respect-use masks a USE='' binpkg when this differs
		net-misc/onedrive libnotify
		# DaVinci Resolve: NVIDIA only (the only VIDEO_CARDS its ebuild needs on this machine)
		media-video/davinci-resolve VIDEO_CARDS: -* nvidia
	EOF2
	# Resolve's installer zip is fetch-restricted and not re-downloadable: back it up (hogsmeade-backup, /etc/hogsmeade/backup-extra.d, one file only)
	insinto /etc/hogsmeade/backup-extra.d
	newins - resolve <<-'EOF2'
		/var/cache/distfiles/DaVinci_Resolve_21.2_Linux.zip
	EOF2
}

pkg_postinst() {
	elog "DaVinci Resolve is fetch-restricted and deliberately NOT a dependency of app-misc/hogsmeade-apps (a missing zip would break image builds)."
	elog "On a reinstall: restore /var/cache/distfiles/DaVinci_Resolve_21.2_Linux.zip from the hogsmeade-backup restic snapshot (restic restore --tag hogsmeade latest --include <that path> --target /) (it is in /etc/hogsmeade/backup-extra.d/resolve),"
	elog "then 'emerge -av media-video/davinci-resolve' (keyword, licence and VIDEO_CARDS come from this package)."
}
