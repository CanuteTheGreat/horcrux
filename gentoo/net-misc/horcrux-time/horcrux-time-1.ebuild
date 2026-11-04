# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux clock rules: chrony steps a months-off clock (makestep 1 -1), rtcsync, UTC hardware clock, sources re-resolved when any network comes up"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	net-misc/chrony
	sys-apps/iproute2
	sys-apps/openrc
"

src_install() {
	exeinto /usr/libexec
	doexe "${FILESDIR}"/horcrux-timesync
	dosbin "${FILESDIR}"/horcrux-time-config
	newinitd "${FILESDIR}"/horcrux-timesync.initd horcrux-timesync
}

pkg_postinst() {
	# chrony.conf and conf.d/hwclock belong to other packages (and to CONFIG_PROTECT): the rules are applied in place, idempotently
	"${EROOT}"/usr/sbin/horcrux-time-config fix "${ROOT%/}" >/dev/null || ewarn "horcrux-time-config: could not apply the clock rules, run: horcrux-time-config fix"
	elog "Clock rules applied to /etc/chrony/chrony.conf (makestep 1 -1, rtcsync) and /etc/conf.d/hwclock (UTC). Check: horcrux-time-config check"
	elog "Enable: rc-update add chronyd default; rc-update add horcrux-timesync default   (the Horcrux installer does it)"
}
