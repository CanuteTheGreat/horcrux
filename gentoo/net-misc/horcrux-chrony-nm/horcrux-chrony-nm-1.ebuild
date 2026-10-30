# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="NetworkManager dispatcher hook: chronyd re-resolves its time sources when a network comes up"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	net-misc/chrony
	net-misc/networkmanager
"

src_install() {
	exeinto /etc/NetworkManager/dispatcher.d
	doexe "${FILESDIR}"/20-horcrux-chrony
}
