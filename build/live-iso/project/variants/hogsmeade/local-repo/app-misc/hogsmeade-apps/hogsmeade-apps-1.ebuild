# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (the owner's personal apps from his Windows install; never in a public image, the Horcrux overlay or a lab/test machine). This
# package is the layer's "set" (RDEPEND = the apps, so @world keeps them current and depclean keeps them) and owns the machine config they need:
# group membership and the pcscd service.
EAPI=8

DESCRIPTION="HOGSMEADE: the owner's apps (AI/dev, hardware, chat, mail, games, media, network, office, security, cloud CLIs, browsers) and their configuration"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-misc/hogsmeade-apps-config
	app-misc/hogsmeade-steam
	app-misc/hogsmeade-microsoft
	dev-util/claude-code
	app-editors/vscode
	dev-util/android-studio
	dev-java/openjdk-bin:25
	sys-apps/ripgrep
	dev-dotnet/dotnet-sdk-bin
	dev-util/github-cli
	dev-db/postgresql
	media-gfx/imagemagick
	app-misc/openrgb
	app-misc/solaar
	games-util/xpadneo
	sys-apps/bolt
	media-libs/libv4l[utils]
	net-print/hplip
	media-sound/easyeffects
	net-irc/halloy
	net-im/teams-for-linux
	net-im/signal-desktop-bin
	net-im/slack
	net-im/zoom
	net-im/proton-meet-bin
	mail-client/thunderbird-bin
	mail-client/proton-mail-bin
	games-emulation/dosbox-staging
	games-util/heroic-bin
	games-util/geforcenow-electron
	games-action/minecraft-launcher
	net-ftp/filezilla
	net-misc/freedownloadmanager
	net-fs/proton-drive-cli-bin
	media-video/makemkv
	media-gfx/loupe
	media-video/vlc
	net-analyzer/ookla-speedtest
	net-analyzer/wireshark
	net-analyzer/mtr
	net-analyzer/tcpdump
	net-dns/bind
	app-office/libreoffice
	app-crypt/yubioath-flutter-bin
	app-crypt/yubikey-manager
	sys-apps/pcsc-lite
	app-crypt/ccid
	sys-auth/proton-authenticator-bin
	app-admin/proton-pass-bin
	net-analyzer/caido-bin
	app-admin/awscli
	app-admin/aws-session-manager-plugin
	net-misc/s3cmd
	net-misc/onedrive
	www-client/google-chrome
	www-client/microsoft-edge
	net-vpn/proton-vpn-gtk-app
"

pkg_postinst() {
	# owner's login(s) (uid >= 1000): the groups the apps need
	local g u
	for g in pcap wireshark plugdev pcscd video; do
		getent group "${g}" >/dev/null || continue
		while IFS=: read -r u _ uid _; do
			[[ ${uid} -ge 1000 && ${uid} -lt 60000 ]] || continue
			id -nG "${u}" | tr ' ' '\n' | grep -qx "${g}" && continue
			gpasswd -a "${u}" "${g}" >/dev/null && elog "added ${u} to ${g}"
		done < <(getent passwd)
	done
	# smart-card daemon for the YubiKey apps (OpenRC)
	if [[ -x /etc/init.d/pcscd && ! -e /etc/runlevels/default/pcscd ]]; then
		rc-update add pcscd default >/dev/null && elog "pcscd enabled in the default runlevel"
	fi
}
