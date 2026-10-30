# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux conversion helpers for the owner's ThinkBook 16 G6 (gringotts): guided install, remote handoff, restore, first boot"
HOMEPAGE="https://git.canutethegreat.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-arch/tar
	app-arch/zstd
	dev-lang/python
	dev-libs/openssl
	net-misc/networkmanager
	net-vpn/tailscale
	sys-apps/coreutils
	sys-apps/openrc
"

src_install() {
	cd "${FILESDIR}" || die
	exeinto /usr/bin
	doexe usr/bin/gringotts-{install,remote,restore,motd}
	exeinto /usr/libexec
	doexe usr/libexec/gringotts-{session,firstboot}
	doinitd etc/init.d/gringotts-firstboot
	insinto /etc/ssh/sshd_config.d
	doins etc/ssh/sshd_config.d/10-gringotts.conf
	insinto /etc/NetworkManager/conf.d
	doins etc/NetworkManager/conf.d/10-horcrux.conf
	insinto /etc/iwd
	doins etc/iwd/main.conf
	insinto /etc/conf.d
	doins etc/conf.d/hostname
	keepdir /var/lib/gringotts
}
