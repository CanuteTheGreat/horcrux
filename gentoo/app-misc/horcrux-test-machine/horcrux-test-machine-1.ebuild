# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Marks a machine as a TEST machine: disk encryption OFF, unattended boot (sets sys-kernel/horcrux-unlock[-encrypted])"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="sys-kernel/horcrux-unlock[-encrypted]"

pkg_preinst() {
	# the installer writes the same file for a fresh test-machine install; replace it instead of leaving a ._cfg file
	rm -f "${EROOT}"/etc/portage/package.use/horcrux-test-machine
}

src_install() {
	insinto /etc/portage/package.use
	newins - horcrux-test-machine <<-'EOF2'
		# owned by app-misc/horcrux-test-machine: this machine boots WITHOUT disk encryption and WITHOUT a passphrase prompt
		sys-kernel/horcrux-unlock -encrypted
	EOF2
	insinto /usr/share/doc/${PF}
	newins - README <<-'EOF2'
		TEST MACHINE: DISK ENCRYPTION IS OFF (owner decision 2026-11-04, only while the machine is used for testing).
		The ZFS pool / XFS+LVM root is not encrypted, the initramfs has no passphrase prompt, the machine boots to the login
		screen unattended (the login screen still asks for the user's password: there is no autologin).

		Turn encryption back on (it cannot be done in place for native ZFS encryption):
		  1. take a fresh restic backup (`ctg-restic backup`) and verify it (`ctg-restic check`),
		  2. emerge --deselect app-misc/horcrux-test-machine; emerge -C app-misc/horcrux-test-machine; emerge -1 sys-kernel/horcrux-unlock
		     (USE=encrypted is the default again) so the unlock module is installed,
		  3. reinstall with the Horcrux installer, encryption = yes (zfs_encrypt / [disk] encryption = true), then restore from restic;
		     for the non-ZFS layout the same applies (XFS on LUKS2).
		Convert the other way (what was done on gringotts/hogshead/madammalkins): zfs send of each dataset (non-raw) to another
		host, recreate the pool unencrypted, receive, regenerate the initramfs. See ctg-infrastructure hosts/<host>/CONVERSION.md.
	EOF2
}
