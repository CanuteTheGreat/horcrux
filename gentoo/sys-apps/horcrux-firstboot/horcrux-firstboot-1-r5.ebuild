# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Horcrux first boot: waits for a network without a time limit (headless-safe, never blocks boot), machine hooks, Portage tree sync"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	sys-apps/iproute2
	|| ( sys-apps/openrc sys-apps/systemd )
	sys-apps/portage
"

src_install() {
	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-firstboot
	newinitd "${FILESDIR}"/horcrux-firstboot.initd horcrux-firstboot
	systemd_dounit "${FILESDIR}"/horcrux-firstboot.service
	keepdir /usr/libexec/horcrux-firstboot.d /var/lib/horcrux
}

pkg_postinst() {
	# Installed onto a host that is already set up (converted in place; the Horcrux database exists): there is no first boot to run.
	# Mark it done so neither the OpenRC script nor the systemd unit starts it and the "first boot finished" invariant holds.
	# Never during an image build (ROOT set): the image's own first boot must still run.
	[[ -n ${ROOT} && ${ROOT} != / ]] && return 0
	"${EROOT:-}"/usr/libexec/horcrux-firstboot --adopt
}
