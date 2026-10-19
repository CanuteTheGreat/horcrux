# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

# Silo: the community-maintained MinIO fork (PGSTY) that still publishes
# release binaries after MinIO stopped shipping them in 2025. Horcrux's S3
# gateway (USE=s3-gateway) runs it; `minio` itself also works if installed.
MY_REL="RELEASE.2026-10-18T00-00-00Z"
MY_PV="20261018000000.0.0"

DESCRIPTION="S3-compatible object storage server (community MinIO fork)"
HOMEPAGE="https://github.com/pgsty/silo"
SRC_URI="
	amd64? ( https://github.com/pgsty/silo/releases/download/${MY_REL}/silo_${MY_PV}_linux_amd64.tar.gz -> ${P}-amd64.tar.gz )
	arm64? ( https://github.com/pgsty/silo/releases/download/${MY_REL}/silo_${MY_PV}_linux_arm64.tar.gz -> ${P}-arm64.tar.gz )
"
S="${WORKDIR}"

LICENSE="AGPL-3"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RESTRICT="bindist mirror strip"

QA_PREBUILT="usr/bin/silo"

src_install() {
	dobin silo
	dodoc README.md NOTICE
}
