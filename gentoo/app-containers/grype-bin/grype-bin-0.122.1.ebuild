# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

# Grype (Anchore): the vulnerability scanner behind Horcrux Security > Vulnerabilities (container images,
# containers, VM guests). ::gentoo only has app-containers/grype-0.86.1, which is ~amd64 only (no arm64) and
# too old for the current vulnerability database schema (`grype db update` refuses it), so the upstream
# release binary is packaged here for amd64 and arm64. Both packages install /usr/bin/grype and block each other.
# The release checksums (grype_<PV>_checksums.txt, signed with sigstore) were compared with the Manifest
# digests when this version was cut; the nvchecker entry bumps it, the Manifest is regenerated per bump.
DESCRIPTION="Vulnerability scanner for container images and filesystems (upstream release binary)"
HOMEPAGE="https://github.com/anchore/grype"
SRC_URI="
	amd64? ( https://github.com/anchore/grype/releases/download/v${PV}/grype_${PV}_linux_amd64.tar.gz -> ${P}-amd64.tar.gz )
	arm64? ( https://github.com/anchore/grype/releases/download/v${PV}/grype_${PV}_linux_arm64.tar.gz -> ${P}-arm64.tar.gz )
"
S="${WORKDIR}"

LICENSE="Apache-2.0"
SLOT="0"
KEYWORDS="amd64 arm64"
RESTRICT="mirror strip"

RDEPEND="!app-containers/grype"

QA_PREBUILT="usr/bin/grype"

src_install() {
	dobin grype
	dodoc README.md CHANGELOG.md
}

pkg_postinst() {
	elog "grype keeps its vulnerability database under \$XDG_CACHE_HOME/grype (Horcrux: <horcrux data dir>/security/scanner-db)."
	elog "Air-gapped nodes: set security vuln_scan.update_db=false and load a downloaded database archive with"
	elog "  grype db import <archive>   (see docs/SECURITY-SUITE.md, \"Offline database\")"
}
