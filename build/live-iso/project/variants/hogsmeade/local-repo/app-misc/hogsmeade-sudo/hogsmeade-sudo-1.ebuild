# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): passwordless sudo for the owner's account.
# Owner rule for images: no default passwordless sudo (public Horcrux images keep the password prompt and root locked); on HOGSMEADE the owner asked
# for it "like it is everywhere else". The drop-in is named zz-* because sudoers.d is read in lexical order and the LAST match wins, and
# /etc/sudoers.d/wheel (%wheel ALL=(ALL:ALL) ALL, from the base image) would otherwise override it. The content has no comment header on purpose: it is
# byte-identical to the file first hand-placed on the machine, so the first emerge adopts it without a ._cfg update.
EAPI=8

DESCRIPTION="HOGSMEADE: passwordless sudo drop-in for the owner's account (zz-canutethegreat)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="app-admin/sudo"

src_install() {
	insinto /etc/sudoers.d
	insopts -m0440
	newins - zz-canutethegreat <<-'EOF2'
		canutethegreat ALL=(ALL) NOPASSWD: ALL
	EOF2
}

pkg_preinst() {
	# a syntax error in a sudoers.d file breaks sudo for everyone: refuse to merge it
	visudo -c -q -f "${ED}/etc/sudoers.d/zz-canutethegreat" || die "visudo rejected zz-canutethegreat"
}
