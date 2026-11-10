# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Renew SSH host and user certificates with a personal CA signer (client tool + daily timer)"
HOMEPAGE="https://git.canutethegreat.com/CanuteTheGreat/ctg-infrastructure"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
IUSE="systemd"

# The CA public keys, signer address and host aliases are NOT here: they come from the private per-machine
# layer (net-misc/ctg-ssh-ca-fleet). Without it this tool does nothing (renew-all exits 0 quietly).
RDEPEND="
	net-misc/curl
	net-misc/openssh
	!systemd? ( virtual/cron )
"

src_install() {
	dobin "${FILESDIR}/ctg-ssh-ca"
	if use systemd; then
		systemd_dounit "${FILESDIR}/ctg-ssh-ca-renew.service" "${FILESDIR}/ctg-ssh-ca-renew.timer"
	else
		# OpenRC (Horcrux): cron.d entry, same daily check as the systemd timer
		insinto /etc/cron.d
		newins "${FILESDIR}/ctg-ssh-ca-renew.cron" ctg-ssh-ca
	fi
}

pkg_postinst() {
	use systemd && elog "systemctl enable --now ctg-ssh-ca-renew.timer   (units ship disabled)"
}
