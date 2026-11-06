# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Encrypted daily restic backup to Backblaze B2, 30-day retention (owner standard), generic host paths (a machine layer adds paths.d/ + required.d/ lists: keyring, browser profiles, app logins); secrets are provisioned after first boot"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
# -personal-backup: the machine has no personal data to back up (app-misc/horcrux-test-machine sets it): the wrapper binaries
# stay, but NO schedule (cron.d file / systemd timers) is installed, so nothing runs and the monitoring has no backup job to report.
IUSE="systemd +personal-backup"

RDEPEND="
	app-backup/restic
	app-arch/zstd
	sys-process/cronie
"

src_install() {
	dosbin "${FILESDIR}"/ctg-restic "${FILESDIR}"/ctg-restic-cron
	insinto /etc/ctg-backup
	doins "${FILESDIR}"/paths "${FILESDIR}"/excludes
	keepdir /etc/ctg-backup/paths.d /etc/ctg-backup/required.d
	fperms 0700 /etc/ctg-backup
	use personal-backup || return 0
	if use systemd; then
		cp "${FILESDIR}"/ctg-restic-{backup,prune,check}.{service,timer} "${T}"/ || die
		sed -i 's:/usr/local/sbin/ctg-restic:/usr/sbin/ctg-restic:' "${T}"/*.service || die
		systemd_dounit "${T}"/ctg-restic-{backup,prune,check}.{service,timer}
		for n in backup prune check; do
			insinto /usr/lib/systemd/system/ctg-restic-${n}.service.d
			newins "${FILESDIR}"/ctg-restic-ac-only.conf ac-only.conf
		done
	else
		insinto /etc/cron.d
		newins "${FILESDIR}"/horcrux-backup.cron horcrux-backup
	fi
}

pkg_postinst() {
	if ! use personal-backup; then
		elog "USE=-personal-backup: no backup schedule installed (test machine)."
		[[ -f ${EROOT}/etc/cron.d/horcrux-backup ]] && ewarn "${EROOT}/etc/cron.d/horcrux-backup is left from an earlier revision: delete it (config-protected files are not removed when edited)."
		return 0
	fi
	elog "Provision the credentials once (from crystalcave, over ssh stdin, root 0600):"
	elog "  /etc/ctg-backup/env      RESTIC_REPOSITORY=s3:https://s3.us-west-002.backblazeb2.com/ctg-backups/<host>, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, RESTIC_PASSWORD_FILE=/etc/ctg-backup/repo.pw"
	elog "  /etc/ctg-backup/repo.pw  repo password (escrow: crystalcave ~/.hermes/secrets/restic/<host>.pw)"
	elog "Until then the cron wrapper logs 'not provisioned' and does nothing."
}
