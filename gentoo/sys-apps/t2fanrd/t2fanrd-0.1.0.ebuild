# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

# Crates list generated with pycargoebuild

EAPI=8

CRATES="
	arraydeque@0.5.1
	cfg-if@1.0.0
	const-random-macro@0.1.16
	const-random@0.1.17
	crunchy@0.2.2
	dlv-list@0.5.2
	getrandom@0.2.12
	glob@0.3.1
	hashbrown@0.14.3
	libc@0.2.152
	nonempty@0.10.0
	once_cell@1.19.0
	ordered-multimap@0.7.1
	proc-macro2@1.0.78
	quote@1.0.35
	rust-ini@0.21.0
	signal-hook-registry@1.4.1
	signal-hook@0.3.17
	syn@2.0.48
	thiserror-impl@1.0.56
	thiserror@1.0.56
	tiny-keccak@2.0.2
	trim-in-place@0.1.7
	unicode-ident@1.0.12
	wasi@0.11.0+wasi-snapshot-preview1
"

COMMIT="48baf962697ec3d4d969c74cf601ee8e15b7aeaa"

inherit cargo

DESCRIPTION="Simple fan daemon for Apple T2 Macs (needs the t2linux applesmc patches)"
HOMEPAGE="https://github.com/GnomedDev/T2FanRD"
SRC_URI="
	https://github.com/GnomedDev/T2FanRD/archive/${COMMIT}.tar.gz -> ${P}.tar.gz
	${CARGO_CRATE_URIS}
"
S="${WORKDIR}/T2FanRD-${COMMIT}"

LICENSE="GPL-3+"
# Dependent crate licenses
LICENSE+=" CC0-1.0 MIT Unicode-DFS-2016"
SLOT="0"
KEYWORDS="~amd64"

QA_FLAGS_IGNORED="usr/bin/t2fanrd"

src_install() {
	cargo_src_install
	newinitd "${FILESDIR}/t2fanrd.initd" t2fanrd
}
