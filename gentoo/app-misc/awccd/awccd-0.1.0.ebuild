# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{12..14} )

inherit python-single-r1 udev

DESCRIPTION="Linux replacement for Alienware Command Center: thermal profiles, fans, AlienFX lighting daemon + GNOME extension"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
IUSE="gnome"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

RDEPEND="
	${PYTHON_DEPS}
	$(python_gen_cond_dep 'dev-python/pygobject:3[${PYTHON_USEDEP}]')
	sys-apps/dbus
	gnome? ( gnome-base/gnome-shell )
"

src_test() {
	cp -r "${FILESDIR}"/awcc "${FILESDIR}"/tests "${T}/" || die
	cd "${T}" && ${EPYTHON} -m unittest discover -s tests || die
}

src_install() {
	python_moduleinto /usr/lib/awccd
	python_domodule "${FILESDIR}/awcc"
	newsbin "${FILESDIR}/awccd-wrapper.py" awccd
	newbin "${FILESDIR}/awccctl-wrapper.py" awccctl
	python_fix_shebang "${ED}/usr/sbin/awccd" "${ED}/usr/bin/awccctl"
	newinitd "${FILESDIR}/awccd.initd" awccd
	newconfd "${FILESDIR}/awccd.confd" awccd
	insinto /etc
	newins "${FILESDIR}/awccd.toml" awccd.toml
	insinto /usr/share/dbus-1/system.d
	doins "${FILESDIR}/org.horcrux.Awcc1.conf"
	udev_dorules "${FILESDIR}/70-alienware-lighting.rules"
	if use gnome; then
		insinto /usr/share/gnome-shell/extensions/awcc@horcrux.errant.solutions
		doins "${FILESDIR}"/extension/{metadata.json,extension.js}
		insinto /usr/share/glib-2.0/schemas
		newins - 50-awcc-horcrux.gschema.override <<-'EOF'
			[org.gnome.shell]
			enabled-extensions=['awcc@horcrux.errant.solutions']
		EOF
	fi
}

pkg_postinst() {
	udev_reload
	elog "rc-update add awccd default && rc-service awccd start   (the Horcrux hogsmeade profile does this)"
	elog "awccctl status | awccctl profile quiet | awccctl light 0000FF shelf alienhead | awccctl light restore"
}
