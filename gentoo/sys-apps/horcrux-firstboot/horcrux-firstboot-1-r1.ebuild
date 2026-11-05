# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux first boot: waits for a network without a time limit (headless-safe, never blocks boot), machine hooks, Portage tree sync"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	sys-apps/iproute2
	sys-apps/openrc
	sys-apps/portage
"

src_install() {
	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-firstboot
	newinitd "${FILESDIR}"/horcrux-firstboot.initd horcrux-firstboot
	keepdir /usr/libexec/horcrux-firstboot.d /var/lib/horcrux
}
