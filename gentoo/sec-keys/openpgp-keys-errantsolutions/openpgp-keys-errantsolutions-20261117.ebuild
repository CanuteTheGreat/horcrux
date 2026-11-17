# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="OpenPGP key that signs Errant Solutions releases (Horcrux, Patronus ISOs, router images, checksums, SBOMs)"
HOMEPAGE="https://errant.solutions/"
S="${WORKDIR}"

LICENSE="public-domain"
SLOT="0"
KEYWORDS="amd64 arm64"

# Fingerprint 5439 3549 39B9 C055 2A68  E9BB E323 A6A3 AC0A 6506 (Errant Solutions Release Signing <security@errant.solutions>).
# Same file as keys/release-signing.asc in the repository and https://<product>.errant.solutions/signing-key.asc.
# Installed where Gentoo's verify-sig.eclass and sec-keys/openpgp-keys-* put keys: /usr/share/openpgp-keys/.
src_install() {
	insinto /usr/share/openpgp-keys
	newins "${FILESDIR}/errantsolutions.asc" errantsolutions.asc
}
