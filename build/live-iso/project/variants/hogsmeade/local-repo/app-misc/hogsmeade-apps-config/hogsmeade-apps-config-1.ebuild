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
	EOF2
	insinto /etc/portage/package.use
	newins - hogsmeade-apps <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-apps-config)
		# Claude Code's prebuilt binary needs AVX/AVX2 (the Alienware's CPU has both)
		dev-util/claude-code cpu_flags_x86_avx cpu_flags_x86_avx2
		media-libs/libv4l utils
		net-dns/avahi python
		sys-libs/zlib minizip
	EOF2
}
