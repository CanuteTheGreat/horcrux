# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="TEST machine profile: disk encryption OFF (unattended boot) and power saving OFF (no lid/idle sleep, no Wi-Fi power save)"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
# One package for the owner's test laptops (gringotts, hogshead, madammalkins): installed ONLY there, never in a public image.
# `emerge -C` it to get normal power saving back (and see the README for the encryption side).
# USE=zfs: the machine has a ZFS root (gringotts, hogshead); without it (madammalkins, XFS) only the power part applies
IUSE="gnome zfs"

RDEPEND="
	zfs? ( sys-kernel/horcrux-unlock[-encrypted] )
	net-wireless/iw
	gnome? ( gnome-base/dconf )
"

pkg_preinst() {
	# the installer writes the same package.use file for a fresh test-machine install; replace it instead of leaving a ._cfg file
	rm -f "${EROOT}"/etc/portage/package.use/horcrux-test-machine
}

src_install() {
	if use zfs; then
		insinto /etc/portage/package.use
		newins - horcrux-test-machine <<-'EOF2'
			# owned by app-misc/horcrux-test-machine: this machine boots WITHOUT disk encryption and WITHOUT a passphrase prompt
			sys-kernel/horcrux-unlock -encrypted
		EOF2
	fi

	insinto /etc/elogind/logind.conf.d
	newins "${FILESDIR}"/logind.conf 50-horcrux-test-machine.conf
	insinto /etc/NetworkManager/conf.d
	newins "${FILESDIR}"/nm.conf 50-horcrux-test-machine.conf
	insinto /etc/modprobe.d
	newins "${FILESDIR}"/modprobe.conf horcrux-test-machine.conf
	insinto /etc/udev/rules.d
	newins "${FILESDIR}"/udev.rules 50-horcrux-test-machine-pm.rules
	if use gnome; then
		insinto /etc/dconf/db/local.d
		newins "${FILESDIR}"/gnome.dconf 10-horcrux-test-machine
	fi
	newinitd "${FILESDIR}"/horcrux-test-power.initd horcrux-test-power

	insinto /usr/share/doc/${PF}
	newins - README <<-'EOF2'
		TEST MACHINE (owner decision 2026-11-04, only while the machine is used for testing)
		  1. DISK ENCRYPTION IS OFF: no passphrase prompt, unattended boot to the login screen (the login screen still asks for the
		     password: there is no autologin). Anyone with the machine can read the disk.
		  2. POWER SAVING IS OFF: lid close / suspend key / idle do not suspend (elogind drop-in), Wi-Fi power save off (NetworkManager,
		     driver options, horcrux-test-power service), no runtime PM on network devices, GNOME/XFCE do not sleep on idle.
		     Critical-battery hibernate (UPower) is deliberately left alone.

		Back to a normal machine: emerge -C app-misc/horcrux-test-machine (power behaviour returns at once after `rc-service
		elogind reload`/next login). Encryption cannot be added in place (native ZFS encryption): take a fresh backup, then reinstall
		with the installer's encryption choice and restore from restic. Conversion notes: ctg-infrastructure hosts/<host>/CONVERSION.md.
	EOF2
}

pkg_postinst() {
	use gnome && has_version gnome-base/dconf && dconf update
	elog "Enable the runtime service: rc-update add horcrux-test-power default && rc-service horcrux-test-power start"
	elog "Apply the logind drop-in without logging anyone out: rc-service elogind reload (or loginctl: kill -HUP the elogind daemon)."
}
