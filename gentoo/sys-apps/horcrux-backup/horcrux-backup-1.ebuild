# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Encrypted daily restic backup to Backblaze B2, 30-day retention (owner standard); secrets are provisioned after first boot"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
IUSE="systemd"

RDEPEND="
	app-backup/restic
	app-arch/zstd
	sys-process/cronie
"

src_install() {
	dosbin "${FILESDIR}"/ctg-restic "${FILESDIR}"/ctg-restic-cron
	insinto /etc/ctg-backup
	doins "${FILESDIR}"/paths "${FILESDIR}"/excludes
	fperms 0700 /etc/ctg-backup
	if use systemd; then
		systemd_dounit "${FILESDIR}"/ctg-restic-{backup,prune,check}.{service,timer}
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
	elog "Provision the credentials once (from crystalcave, over ssh stdin, root 0600):"
	elog "  /etc/ctg-backup/env      RESTIC_REPOSITORY=s3:https://s3.us-west-002.backblazeb2.com/ctg-backups/<host>, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, RESTIC_PASSWORD_FILE=/etc/ctg-backup/repo.pw"
	elog "  /etc/ctg-backup/repo.pw  repo password (escrow: crystalcave ~/.hermes/secrets/restic/<host>.pw)"
	elog "Until then the cron wrapper logs 'not provisioned' and does nothing."
}
