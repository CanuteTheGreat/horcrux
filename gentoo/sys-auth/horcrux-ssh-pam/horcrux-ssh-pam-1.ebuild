# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux: ssh logins do not start or hold the user's OpenRC user services (sshd PAM stack without pam_openrc)"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# pam_openrc counts every PAM login of a user and stops the user's OpenRC user runlevel (user.<name>: the default user runlevel plus the GNOME
# session's gnome-shell, gsd-*, ...) only with the LAST one. An ssh login therefore kept a logged-out GNOME session's services running, and
# supervise-daemon respawned gnome-shell into the closed session: the next GNOME login on that seat failed with mutter
# "Failed to take control of the session: EBUSY" and had no mouse and keyboard (HOGSMEADE, 2026-11-13; agents ssh in as the desktop user).
# gnome-base/gnome-session-openrc-49.2-r2::horcrux makes the GNOME session own its services; this package additionally keeps ssh out of the
# user runlevel entirely, on every edition (server too: an ssh login never starts the user's desktop/user services).
# OpenSSH >= 10.0 has PAMServiceName; the drop-in sorts before Gentoo's 9999999gentoo-pam.conf (sshd: first value wins).
RDEPEND="
	>=net-misc/openssh-10.0[pam]
	sys-auth/pambase
	sys-libs/pam
"

src_install() {
	insinto /etc/pam.d
	newins "${FILESDIR}"/sshd-horcrux.pam sshd-horcrux
	insinto /etc/ssh/sshd_config.d
	doins "${FILESDIR}"/10-horcrux-ssh-pam.conf
	dosbin "${FILESDIR}"/horcrux-ssh-pam-check
}

pkg_postinst() {
	"${EROOT}"/usr/sbin/horcrux-ssh-pam-check "${ROOT%/}" || ewarn "horcrux-ssh-pam-check: the sshd PAM stack drifted from pambase (see above)"
	elog "New ssh logins use /etc/pam.d/sshd-horcrux (no OpenRC user services). sshd reads it for each new connection; existing sessions are unchanged."
}
