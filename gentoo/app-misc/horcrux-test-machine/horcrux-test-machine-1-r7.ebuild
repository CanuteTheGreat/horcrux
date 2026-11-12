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
# +plain: this machine boots WITHOUT disk encryption (ZFS root or XFS/LVM): unlock module off, resume + zfs/lvm conf only. Enable it ONLY after the disks are really unencrypted (-plain while a LUKS root still exists, or the machine cannot boot).
# +gnome (gringotts, hogshead) / xfce (madammalkins, per-login xfconf power settings: the XFCE defaults file is owned by horcrux-surface-desktop).
IUSE="+gnome +plain smallram xfce"

RDEPEND="
	plain? ( sys-kernel/horcrux-unlock[-encrypted] )
	net-wireless/iw
	sys-apps/ethtool
	net-misc/networkmanager
	gnome? ( gnome-base/dconf )
	xfce? ( xfce-base/xfconf )
	dev-lang/python
	!net-misc/nextcloud-client
	!net-im/discord
	!net-im/signal-desktop
	!net-im/signal-desktop-bin
	!media-sound/spotify
	!app-admin/keepassxc
	!app-admin/proton-pass-bin
	!net-vpn/proton-vpn-gtk-app
	!app-antivirus/clamav
	!app-misc/gringotts-config
	!app-misc/gringotts-tools
"
# The blockers above are the Portage-level half of the guard: a test machine can't even be updated into carrying the owner's
# personal apps (weak blockers unmerge them). The full list is files/personal-apps.denylist; horcrux-test-machine-check (lab hx-invariants) enforces it.

pkg_preinst() {
	# the installer writes the same package.use file for a fresh test-machine install; replace it instead of leaving a ._cfg file
	rm -f "${EROOT}"/etc/portage/package.use/horcrux-test-machine
}

src_install() {
	insinto /etc/portage/package.use
	newins - horcrux-test-machine <<-'EOF2'
		# owned by app-misc/horcrux-test-machine
		# no personal data on a test machine: no personal backup schedule (horcrux-backup installs no cron.d/timers), so no backup job exists to report
		sys-apps/horcrux-backup -personal-backup
	EOF2
	if use plain; then
		cat >> "${ED}"/etc/portage/package.use/horcrux-test-machine <<-'EOF2' || die
			# this machine boots WITHOUT disk encryption and WITHOUT a passphrase prompt
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
	if use xfce; then
		exeinto /usr/libexec
		newexe "${FILESDIR}"/xfce-power.sh horcrux-test-xfce-power
		insinto /etc/xdg/autostart
		newins "${FILESDIR}"/xfce-power.desktop horcrux-test-xfce-power.desktop
	fi
	insinto /etc/elogind/sleep.conf.d
	newins "${FILESDIR}"/sleep.conf 50-horcrux-test-machine.conf
	insinto /etc/sysctl.d
	newins "${FILESDIR}"/sysctl.conf 50-horcrux-test-machine.conf
	exeinto /usr/libexec
	newexe "${FILESDIR}"/horcrux-test-watchdog.sh horcrux-test-watchdog
	newinitd "${FILESDIR}"/horcrux-test-watchdog.initd horcrux-test-watchdog
	newinitd "${FILESDIR}"/horcrux-test-power.initd horcrux-test-power
	newinitd "${FILESDIR}"/horcrux-test-klog.initd horcrux-test-klog
	newinitd "${FILESDIR}"/horcrux-test-netwatch.initd horcrux-test-netwatch
	exeinto /usr/libexec
	newexe "${FILESDIR}"/horcrux-test-netwatch.sh horcrux-test-netwatch
	if use smallram; then
		insinto /etc/portage/env
		newins "${FILESDIR}"/smallram.env horcrux-smallram.conf
		insinto /etc/portage/package.env
		newins "${FILESDIR}"/smallram.package.env horcrux-test-machine
	fi
	# no-personal-apps guard (owner order 2026-11-05): policy data + the checker the lab's hx-invariants job runs
	insinto /usr/share/${PN}
	doins "${FILESDIR}"/personal-apps.denylist "${FILESDIR}"/personal-paths.denylist "${FILESDIR}"/world.allowlist
	dosbin "${FILESDIR}"/horcrux-test-machine-check
	insinto /etc/logrotate.d
	newins "${FILESDIR}"/logrotate.conf horcrux-test-machine

	insinto /usr/share/doc/${PF}
	newins - README <<-'EOF2'
		TEST MACHINE (owner decision 2026-11-04, only while the machine is used for testing)
		  1. DISK ENCRYPTION IS OFF: no passphrase prompt, unattended boot to the login screen (the login screen still asks for the
		     password: there is no autologin). Anyone with the machine can read the disk.
		  2. POWER SAVING IS OFF: the machine can NEVER be put to sleep (elogind sleep.conf AllowSuspend=no etc. refuses even a
		     desktop session's own idle timer), lid/suspend key/idle do nothing (elogind drop-in), Wi-Fi power save off (NetworkManager,
		     driver options, horcrux-test-power service), no runtime PM on network devices, GNOME/XFCE do not sleep on idle.
		     Critical-battery hibernate (UPower) is deliberately left alone.

		   A hung machine reboots itself: hardware watchdog (horcrux-test-watchdog, petted only while a root-fs write completes) +
		   kernel.panic=20 / softlockup_panic, so nobody has to power-cycle a test laptop.

		3. NO PERSONAL APPS, NO PERSONAL DATA (owner order 2026-11-05): only the public Horcrux editions + test tooling + the machine's
		   hardware enablement. `horcrux-test-machine-check` lists violations (denylist of personal apps/paths + an ALLOWLIST for the
		   world file: /usr/share/horcrux-test-machine/). To add a legitimate package, add it to world.allowlist in the same change.

		4. NO PERSONAL BACKUP: sys-apps/horcrux-backup is built with -personal-backup (package.use/horcrux-test-machine), so no restic cron/timer
		   exists on a test machine (it has no personal data; the lab's own copies are not personal backups).

		Back to a normal machine: emerge -C app-misc/horcrux-test-machine (power behaviour returns at once after `rc-service
		elogind reload`/next login). Encryption cannot be added in place (native ZFS encryption): take a fresh backup, then reinstall
		with the installer's encryption choice and restore from restic. Conversion notes: ctg-infrastructure hosts/<host>/CONVERSION.md.
	EOF2
}

pkg_postinst() {
	# Portage decides config protection BEFORE pkg_preinst runs, so the rm above does not stop it: on an update the new
	# package.use file lands as ._cfgNNNN_horcrux-test-machine and the real file stays deleted (the machine would then
	# pull the encrypted horcrux-unlock). The file is package-owned and meant to be replaced: move it into place.
	local c d="${EROOT}"/etc/portage/package.use
	for c in "${d}"/._cfg????_horcrux-test-machine; do
		[[ -f ${c} ]] && mv -f "${c}" "${d}"/horcrux-test-machine
	done
	use gnome && has_version gnome-base/dconf && dconf update
	if [[ -z ${ROOT} || ${ROOT} == / ]]; then
		local s
		for s in horcrux-test-power horcrux-test-klog horcrux-test-netwatch horcrux-test-watchdog; do
			rc-update add ${s} default >/dev/null 2>&1
		done
		elog "Enabled in the default runlevel: horcrux-test-power horcrux-test-klog horcrux-test-netwatch horcrux-test-watchdog (start them now with rc-service)"
	fi
	elog "Apply the logind drop-in without logging anyone out: rc-service elogind reload (or loginctl: kill -HUP the elogind daemon)."
}
