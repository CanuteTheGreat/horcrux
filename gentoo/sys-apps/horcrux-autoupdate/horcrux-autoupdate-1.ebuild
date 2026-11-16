# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Automatic Portage updates for every Horcrux install: daily sync + @world from the binhost, boot-environment snapshot first, status file, never reboots"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
IUSE="systemd"

# Pulled in by sys-apps/horcrux-core, so it is on every desktop and server install, and ON by default (ENABLED=no in /etc/horcrux/autoupdate.conf turns it off).
# Scheduling, one per init (OWNER RULE: OpenRC and systemd equally): OpenRC = /etc/cron.d (cronie, already required by horcrux-backup), systemd = a timer
# enabled at install. Both only trigger hourly: the program decides when an update is due, so a laptop that was off at night catches up.
# Never reboots: a kernel/microcode update is recorded in /var/lib/horcrux-autoupdate/reboot-required and /run/reboot-required.
RDEPEND="
	dev-lang/python
	sys-apps/portage
	sys-apps/util-linux
	!systemd? ( sys-process/cronie )
"

src_install() {
	dosbin "${FILESDIR}"/horcrux-autoupdate
	insinto /etc/horcrux
	doins "${FILESDIR}"/autoupdate.conf
	keepdir /var/lib/horcrux-autoupdate /var/log/horcrux-autoupdate
	if use systemd; then
		systemd_dounit "${FILESDIR}"/horcrux-autoupdate.service "${FILESDIR}"/horcrux-autoupdate.timer
	else
		insinto /etc/cron.d
		newins "${FILESDIR}"/horcrux-autoupdate.cron horcrux-autoupdate
	fi
}

pkg_postinst() {
	if use systemd; then
		systemd_enable_service timers.target horcrux-autoupdate.timer
		elog "horcrux-autoupdate.timer is enabled (hourly trigger, the update runs once per INTERVAL_HOURS)."
	else
		elog "Scheduled through /etc/cron.d/horcrux-autoupdate: make sure cronie runs (rc-update add cronie default)."
	fi
	elog "Status: horcrux-autoupdate status | check. Settings: /etc/horcrux/autoupdate.conf. It never reboots: see /var/lib/horcrux-autoupdate/reboot-required."
}
