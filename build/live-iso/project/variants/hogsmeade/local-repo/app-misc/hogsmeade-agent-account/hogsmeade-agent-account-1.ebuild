# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): a dedicated login for the Hermes agent.
# Why: agents used to ssh in as the owner's own account (canutethegreat). Every such login was a pam_openrc session of HIS user, which kept his
# OpenRC user runlevel alive across a GNOME logout; supervise-daemon then respawned gnome-shell into the closed session and his next login had no
# mouse and keyboard (2026-11-13). The agent now has its own account: user acct-user/hermes-agent, ssh with a personal-CA certificate whose
# principal is "hermes-agent" (no authorized_keys, no password), passwordless sudo through this package's zz-hermes-agent drop-in (sudoers.d is
# read in lexical order and the last match wins, so zz- comes after the image's wheel rule). Agents use `sudo -u canutethegreat` only for
# read-only checks of his files; nothing runs inside his desktop session.
EAPI=8

DESCRIPTION="HOGSMEADE: dedicated Hermes agent login (acct-user/hermes-agent + passwordless sudo)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	acct-user/hermes-agent
	app-admin/sudo
	net-misc/ctg-ssh-ca-fleet
"

src_install() {
	insinto /etc/sudoers.d
	insopts -m0440
	newins - zz-hermes-agent <<-'EOF2'
		hermes-agent ALL=(ALL) NOPASSWD: ALL
	EOF2
}

pkg_preinst() {
	# a syntax error in a sudoers.d file breaks sudo for everyone: refuse to merge it
	visudo -c -q -f "${ED}/etc/sudoers.d/zz-hermes-agent" || die "visudo rejected zz-hermes-agent"
}
