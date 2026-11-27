# Copyright 1999-2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

# Horcrux copy of ::gentoo sys-cluster/sanlock-5.1.0-r3, unchanged apart from
# the OpenRC scripts (files/sanlock.initd, files/wdmd.initd): ::gentoo's point
# pidfile at /var/run/*.pid, but sanlock and wdmd write /run/sanlock/sanlock.pid
# and /run/wdmd/wdmd.pid, so OpenRC reports a running daemon "crashed" and a
# restart kills the lockspaces (found by the SAN vmtest on an installed node).
# The -r100 revision keeps it ahead of ::gentoo. When ::gentoo fixes the
# scripts or bumps sanlock, re-check and delete this copy.

EAPI=8

inherit flag-o-matic linux-info systemd

DESCRIPTION="shared storage lock manager"
HOMEPAGE="https://codeberg.org/sanlock/sanlock"
SRC_URI="https://codeberg.org/${PN}/${PN}/releases/download/${P}/${P}.tar.gz"

LICENSE="LGPL-2+ GPL-2 GPL-2+"
SLOT="0"
KEYWORDS="amd64 arm64"
IUSE="selinux"

DEPEND="
	dev-libs/libaio
	sys-apps/util-linux
	sys-fs/lvm2
"
RDEPEND="
	acct-user/${PN}
	acct-group/${PN}
	selinux? (
		sec-policy/selinux-sanlock
	)
	${DEPEND}
"

PATCHES=(
	"${FILESDIR}/sanlock-3.8.4-implicit-func-decls.patch"
)

CONFIG_CHECK="~SOFT_WATCHDOG"

src_compile() {
	# -Werror=lto-type-mismatch
	# https://bugs.gentoo.org/863734
	# https://pagure.io/sanlock/issue/10
	filter-lto

	for d in wdmd src; do
		emake -C ${d}
	done
}

src_install() {
	for d in wdmd src; do
		emake -C ${d} DESTDIR="${D}" LIBDIR="${EPREFIX}/usr/$(get_libdir)" install
	done

	keepdir /var/lib/sanlock

	# config
	dodir /etc/wdmd.d
	dodir /etc/sanlock
	insinto /etc/sanlock
	doins src/sanlock.conf

	# init
	newconfd init.d/sanlock.sysconfig sanlock
	newconfd init.d/wdmd.sysconfig wdmd
	newinitd "${FILESDIR}"/sanlock.initd sanlock
	newinitd "${FILESDIR}"/wdmd.initd wdmd

	# systemd
	local utildir="$(systemd_get_utildir)"
	exeinto "${utildir#"${EPREFIX}"}"
	doexe init.d/systemd-wdmd
	systemd_dounit init.d/sanlock.service
	systemd_dounit init.d/wdmd.service
}
