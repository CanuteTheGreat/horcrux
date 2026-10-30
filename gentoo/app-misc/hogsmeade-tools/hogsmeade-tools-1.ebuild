# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="HOGSMEADE (Alienware 18 Area-51) tools: installer front door, hardware profile, first-boot verifier, snapshots, backup, health, remote handoff"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="
	app-backup/restic
	app-shells/bash
	x11-libs/libnotify
	net-vpn/tailscale
	sys-apps/nvme-cli
	sys-apps/rsync
	sys-apps/smartmontools
	sys-fs/cryptsetup
	sys-fs/zfs
	sys-power/switcheroo-control
	sys-process/cronie
	x11-apps/mesa-progs
"

src_install() {
	dobin "${FILESDIR}"/{prime-run,hogsmeade-verify,hogsmeade-install,hogsmeade-remote}
	dosbin "${FILESDIR}"/{hogsmeade-health,hogsmeade-snapshots,hogsmeade-backup}
	newinitd "${FILESDIR}/hogsmeade-firstboot.initd" hogsmeade-firstboot
	insinto /usr/share/live-installer/hwprofiles
	doins "${FILESDIR}/hogsmeade.toml"
	insinto /etc/modprobe.d
	newins "${FILESDIR}/nvidia-hogsmeade.conf" nvidia-hogsmeade.conf
	newins "${FILESDIR}/zfs-hogsmeade.conf" zfs-hogsmeade.conf
	insinto /etc/cron.d
	newins "${FILESDIR}/hogsmeade.cron" hogsmeade
	insinto /usr/share/hogsmeade
	doins "${FILESDIR}/POST-INSTALL.md"
	insinto /etc/hogsmeade
	newins - README <<-'EOF'
		backup.env   RESTIC_REPOSITORY / AWS_* / RESTIC_PASSWORD_FILE for hogsmeade-backup (installed by the private package)
		ts-authkey   one-time Tailscale handoff key (private package)
	EOF
}
