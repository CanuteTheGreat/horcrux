# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): the idle gate + guard for the CI runner guests the lab scheduler
# runs here when he is away (owner 2026-11-13: use idle hardware, no end date). A guest may run only when: he has had no input for 15+ min (gnome-shell idle monitor
# AND logind IdleHint), no game, GPU/ComfyUI idle, CPU pressure ~0, and >= 16 GB RAM stay free; at most half the cores; no GPU passthrough. Init-neutral (OpenRC + systemd).
EAPI=8

DESCRIPTION="HOGSMEADE: owner-idle gate and drain guard for CI runner guests"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-containers/docker
	sys-apps/util-linux
	sys-apps/gawk
	net-misc/curl
	dev-libs/glib
"

src_install() {
	exeinto /usr/libexec
	doexe "${FILESDIR}"/hogsmeade-ci-gate "${FILESDIR}"/hogsmeade-ci-guard
	newinitd "${FILESDIR}"/hogsmeade-ci-guard.initd hogsmeade-ci-guard
	newconfd "${FILESDIR}"/hogsmeade-ci-guard.confd hogsmeade-ci-guard
	insinto /usr/lib/systemd/system
	doins "${FILESDIR}"/hogsmeade-ci-guard.service
}

pkg_postinst() {
	elog "Enable: rc-update add hogsmeade-ci-guard default && rc-service hogsmeade-ci-guard start (systemd: systemctl enable --now hogsmeade-ci-guard)"
	elog "The guests themselves are started by the lab scheduler (crystalcave ~/.hermes/scripts/lab/cirunner/hogsmeade.py) through 'sudo -n hogsmeade-ci-gate'."
}
