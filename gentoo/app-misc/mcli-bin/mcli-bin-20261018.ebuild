# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

# mcli: the MinIO client from the community fork (PGSTY), installed as
# `mcli` so it never collides with app-misc/mc (Midnight Commander).
# Horcrux's S3 gateway (USE=s3-gateway) administers its server with it.
MY_REL="RELEASE.2026-10-18T00-00-00Z"
MY_PV="20261018000000.0.0"

DESCRIPTION="MinIO client (mcli) for S3-compatible object storage"
HOMEPAGE="https://github.com/pgsty/mc"
SRC_URI="
	amd64? ( https://github.com/pgsty/mc/releases/download/${MY_REL}/mcli_${MY_PV}_linux_amd64.tar.gz -> ${P}-amd64.tar.gz )
	arm64? ( https://github.com/pgsty/mc/releases/download/${MY_REL}/mcli_${MY_PV}_linux_arm64.tar.gz -> ${P}-arm64.tar.gz )
"
S="${WORKDIR}"

LICENSE="AGPL-3"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RESTRICT="bindist mirror strip"

QA_PREBUILT="usr/bin/mcli"

src_install() {
	dobin mcli
	dodoc README.md
}
