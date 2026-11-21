# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_{13..14} )
inherit python-single-r1

DESCRIPTION="Declarative machine profiles for Horcrux: plan/apply/generations/rollback, plus the hardware layer definitions"
HOMEPAGE="https://horcrux.errant.solutions/"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

# One stock installer ISO for every machine: the machine itself is a profile (TOML) applied by this program, first by the live installer
# inside the new system, later by the administrator after editing the profile (`horcrux-profile plan`, `horcrux-profile apply`).
# files/horcrux_profile.py is byte-identical to build/live-iso/common/overlay/usr/lib/live-installer/horcrux_profile.py (the copy the
# installer runs); build/live-iso/common/tests/test_machine_profile.py fails when they differ. files/hwlayers/*.toml likewise equal the
# ISO's build/live-iso/project/overlay/usr/share/horcrux/hwlayers/.
RDEPEND="${PYTHON_DEPS}"

src_install() {
	python_scriptinto /usr/bin
	python_newscript "${FILESDIR}"/horcrux_profile.py horcrux-profile
	insinto /usr/share/horcrux/hwlayers
	doins "${FILESDIR}"/hwlayers/*.toml
	keepdir /var/lib/horcrux/profile
	fperms 0700 /var/lib/horcrux/profile
}
