# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the owner's personal + work Microsoft 365 access. GNOME Online Accounts 'Microsoft 365' + the Files (gvfs) OneDrive mount,
# and the abraunegg onedrive client as two OpenRC user services (onedrive.personal, onedrive.work) with their own config dirs.
# Since -r2 also the OneDrive "known folders" (owner 2026-11-13: on Windows his Desktop/Documents/Downloads/Pictures/Videos... WERE OneDrive Personal):
# hogsmeade-known-folders (login autostart, as the user) links ~/<Folder> -> ~/OneDrive-Personal/<Folder> and writes user-dirs.dirs. It never moves or
# deletes files: a real folder with content is reported and left alone.
# This package is the layer's "set" for it (RDEPEND = the software, so @world keeps it); its USE flags live in app-misc/hogsmeade-microsoft-config.
EAPI=8

DESCRIPTION="HOGSMEADE: Microsoft 365 (GNOME Online Accounts, Files OneDrive, onedrive sync services)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

# the USE requirements mirror hogsmeade-microsoft-config's package.use
RDEPEND="
	app-misc/hogsmeade-microsoft-config
	net-libs/gnome-online-accounts[ms365]
	gnome-base/gvfs[onedrive,gnome-online-accounts]
	>=net-misc/onedrive-2.5.11[libnotify]
	sys-apps/openrc
	x11-misc/xdg-user-dirs
"

src_install() {
	exeinto /etc/user/init.d
	newexe "${FILESDIR}"/onedrive.initd onedrive
	dosym onedrive /etc/user/init.d/onedrive.personal
	dosym onedrive /etc/user/init.d/onedrive.work
	dobin "${FILESDIR}"/onedrive-login
	insinto /usr/share/hogsmeade-microsoft
	doins "${FILESDIR}"/config-personal "${FILESDIR}"/config-work
	dobin "${FILESDIR}"/hogsmeade-known-folders
	insinto /etc/xdg/autostart
	doins "${FILESDIR}"/hogsmeade-known-folders.desktop
}

pkg_postinst() {
	# the owner's login(s) (uid >= 1000): seed each account's config dir once (never overwrite)
	local u uid home a
	while IFS=: read -r u _ uid _ _ home _; do
		[[ ${uid} -ge 1000 && ${uid} -lt 60000 && -d ${home} ]] || continue
		for a in personal work; do
			[[ -e ${home}/.config/onedrive-${a}/config ]] && continue
			mkdir -p "${home}/.config" && install -d -o "${u}" -g "$(id -gn "${u}")" -m 0700 "${home}/.config/onedrive-${a}" &&
				install -o "${u}" -g "$(id -gn "${u}")" -m 0600 "${EROOT}"/usr/share/hogsmeade-microsoft/config-${a} "${home}/.config/onedrive-${a}/config" &&
				elog "seeded ${home}/.config/onedrive-${a}/config for ${u}"
		done
	done < <(getent passwd)
	elog "Authorise each account once, as yourself, in a terminal in your desktop session: 'onedrive-login personal' and 'onedrive-login work' (the browser opens, onedrive catches the answer on a local loopback listener; no copy-paste)."
	elog "Home folders: at each login hogsmeade-known-folders links ~/Documents, ~/Downloads, ~/Pictures ... into ~/OneDrive-Personal (as on Windows)."
	elog "It never moves or deletes files; a real folder with content is left alone and noted in ~/.local/state/hogsmeade-known-folders.log."
	elog "The services onedrive.personal / onedrive.work stay stopped until then. Settings > Online Accounts > Microsoft 365 needs a log-out/in (goa-daemon is already running)."
}
