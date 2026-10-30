# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

MY_P="rustinion-linux-x86_64-${PV}"

DESCRIPTION="Rustinion cross-platform app/OS-update agent (resident mode, OpenRC service)"
HOMEPAGE="https://rustinion.com"
SRC_URI="https://rustinion.com/downloads/${MY_P}"
S="${WORKDIR}"

LICENSE="all-rights-reserved"
RESTRICT="mirror bindist strip"
SLOT="0"
KEYWORDS="-* ~amd64"

QA_PREBUILT="usr/bin/rustinion"

src_unpack() { :; }

src_install() {
	newbin "${DISTDIR}/${MY_P}" rustinion
	newinitd "${FILESDIR}/rustinion.initd" rustinion
}

pkg_postinst() {
	elog "Enrol this machine with your tenant token, then start the service:"
	elog "  rustinion enroll --token <token>   (identity lives in /etc/rustinion, restore it from backup if you have it)"
	elog "  rc-update add rustinion default && rc-service rustinion start"
	elog "Update ownership: Portage owns /usr/bin/rustinion (emerge -u app-admin/rustinion-bin). The agent's own"
	elog "signed self-update may replace the binary between emerges; the next emerge restores the packaged version."
}
