# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit systemd

DESCRIPTION="Rustinion agent (device management and monitoring, OpenRC and systemd)"
HOMEPAGE="https://rustinion.com"
SRC_URI="
	amd64? ( https://rustinion.com/downloads/rustinion-linux-x86_64-${PV} )
	arm64? ( https://rustinion.com/downloads/rustinion-linux-arm64-${PV} )
"
S="${WORKDIR}"

LICENSE="all-rights-reserved"
SLOT="0"
KEYWORDS="-* ~amd64 ~arm64"
RESTRICT="mirror bindist strip"

# rustinion-bin (removed from this overlay) owned the same /usr/bin/rustinion and init script: the blocker makes
# portage unmerge it in the same transaction (tested: bin-0.1.307 -> agent-0.1.318, enrolment untouched).
# pkgcheck NonexistentBlocker on this line is expected: the overlay root is a subdirectory of the git repo.
RDEPEND="!app-admin/rustinion-bin"

QA_PREBUILT="usr/bin/rustinion"

src_unpack() {
	# a single prebuilt static binary (signed by the Rustinion release key upstream; the Manifest pins its hashes)
	:
}

src_install() {
	local bin
	use amd64 && bin="rustinion-linux-x86_64-${PV}"
	use arm64 && bin="rustinion-linux-arm64-${PV}"
	newbin "${DISTDIR}/${bin}" rustinion

	newinitd "${FILESDIR}/rustinion.initd" rustinion
	systemd_dounit "${FILESDIR}/rustinion.service"

	# marker: the agent never replaces this binary itself (see pkg_managed.rs)
	insinto /usr/lib/rustinion
	newins - package-managed <<<"portage"

	keepdir /etc/rustinion
}

pkg_postinst() {
	if [[ -f "${EROOT}/etc/rustinion/device.json" ]]; then
		elog "This machine is already enrolled (/etc/rustinion/device.json is kept)."
		elog "Enable the service: rc-update add rustinion default && rc-service rustinion start"
	else
		elog "Enrol this machine with your tenant token, then start the service:"
		elog "  rustinion enroll --token <token>"
		elog "  rc-update add rustinion default && rc-service rustinion start   (systemd: systemctl enable --now rustinion)"
	fi
	elog "Portage owns /usr/bin/rustinion: update with emerge -u app-admin/rustinion-agent."
}
