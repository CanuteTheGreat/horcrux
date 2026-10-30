# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="dracut module: one passphrase for LUKS2 hibernation swap and native-encrypted ZFS root, resume before pool import"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="
	sys-kernel/dracut
	sys-fs/cryptsetup
	sys-fs/zfs
"

src_install() {
	exeinto /usr/lib/dracut/modules.d/91horcrux-unlock
	doexe "${FILESDIR}"/*.sh
	insinto /etc/dracut.conf.d
	newins - 91-horcrux-unlock.conf <<-'EOF'
		# Horcrux: LUKS2 hibernation swap + ZFS with ONE passphrase (dracut module horcrux-unlock)
		add_dracutmodules+=" crypt dm resume horcrux-unlock zfs "
		add_drivers+=" dm_crypt "
	EOF
}
