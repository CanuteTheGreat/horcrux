# Copyright 2026 Errant Solutions
# PRIVATE, machine-local package (never in git, never in a public ISO): the owner's handoff key, the agent's ssh public key and
# the backup repository credentials of HOGSMEADE. The payload is copied into files/ by the private image build only.
EAPI=8

DESCRIPTION="HOGSMEADE private payload: Tailscale handoff key, agent ssh key, restic backup credentials"
S="${WORKDIR}"
LICENSE="all-rights-reserved"
RESTRICT="bindist mirror"
SLOT="0"
KEYWORDS="~amd64"

src_install() {
	insinto /etc/hogsmeade
	insopts -m0600
	local f
	for f in ts-authkey backup.env restic-password; do
		[[ -s ${FILESDIR}/${f} ]] && newins "${FILESDIR}/${f}" "${f}"
	done
	# the agent's key: root (key-only sshd); hogsmeade-firstboot copies it to the desktop user once that user exists
	if [[ -s ${FILESDIR}/agent_authorized_keys ]]; then
		newins "${FILESDIR}/agent_authorized_keys" agent_authorized_keys
		insinto /root/.ssh
		newins "${FILESDIR}/agent_authorized_keys" authorized_keys
	fi
}

pkg_postinst() {
	chmod 0700 "${ROOT}"/root/.ssh 2>/dev/null
}
