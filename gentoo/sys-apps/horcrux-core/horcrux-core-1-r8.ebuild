# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit udev

DESCRIPTION="Horcrux core: the headless hypervisor/server base every Horcrux install is made of (clock, first boot, backup, overlay sync, ZFS rules); desktops depend on it"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# LAYERING (owner 2026-11-03: "It should work on a headless server too. It shouldn't be a desktop-only OS."):
#   core            this package: server + hypervisor, NO display stack, NO desktop, NO machine-specific app (steam, rustinion, ... live in a machine layer)
#   desktop edition gnome-extra/horcrux-gnome-defaults (GNOME) depends on THIS package; the XFCE edition for small devices does the same
#   machine layers  the owner's own machines (gringotts, hogshead, surface, owner-ws) add their apps on top of an edition
# Every fix that applies to every Horcrux install belongs here (or in a package listed here), never in an edition: the 2026-11-03 gringotts fixes (clock
# hook, first-boot network wait, kept dev files, zfs dist-kernel USE) had been attached to the GNOME package and a server install would have missed them.
# sys-apps/horcrux-release: the package-owned /usr/lib/os-release (name, version, build id, variant); every Horcrux install has it.
# dev-util/pahole:
#   an installed system must be able to build an out-of-tree kernel module (zfs-kmod after every dist-kernel update, DKMS-style drivers) against its running kernel:
#   gentoo-kernel-bin modules carry BTF, which `make modules` finishes with pahole. It used to be on an image only when the zfs-kmod binary package of the day happened
#   to be built from source (QEMU gate kernel-module-builds-against-running-kernel failed on a server rebuilt from the package cache, 2026-11-04).
# /etc/docker/daemon.json ("ip-forward-no-drop"):
#   Docker >= 28 sets the iptables FORWARD policy to DROP when it enables IP forwarding. nftables evaluates EVERY base chain a packet hits, so that DROP
#   also kills traffic libvirt/Horcrux already accepted in its own table: every NAT-network VM (virbr0, vmbr*) lost the internet whenever dockerd ran
#   (owner-ws 2026-11-12: example-vm could not reach ProtonVPN). With this option Docker leaves the policy alone; it still isolates its own
#   bridges (DOCKER-FORWARD / DOCKER-ISOLATION / per-network DROP of unpublished ports). The file is harmless where Docker is not installed
#   (QEMU gate docker-does-not-break-vm-nat-forwarding checks it everywhere and does the netns forwarding test where Docker exists).
# /etc/rc.conf.d/horcrux-containerd.conf (-r4): containerd_socket_delay=120. OpenRC's containerd script gives the daemon 5 s to create its socket, else the service
#   is marked failed with the daemon still running, and dockerd never starts ("containerd is already running ... cannot start docker as containerd would not
#   start"). Reproduced on the aarch64 TCG zfs-mirror install (live-iso runs 2877/3005: docker-does-not-break-vm-nat-forwarding DOCKER-DOWN). rc.conf.d/*.conf is
#   sourced by every OpenRC service script, so the app-containers/containerd-owned /etc/conf.d/containerd stays untouched. Harmless on systemd.
# sys-apps/horcrux-autoupdate (-r5): daily `emaint sync` + `emerge -uDN @world` (binhost first), snapshot before, never reboots. Nothing ran `emerge @world` on an
#   installed machine, so every package (browser, password manager, microcode, grub, xz) stayed at its install-day version until someone updated by hand.
# sys-auth/horcrux-ssh-pam (-r2): ssh logins do not start or hold the user's OpenRC user services (pam_openrc stops them only with the user's LAST
#   login, so an ssh session kept a logged-out GNOME session's gnome-shell alive; also keeps servers' ssh logins from spawning user services).
# /usr/lib/udev/rules.d/80-horcrux-udisks-hide.rules: UDISKS_IGNORE for ZFS pool members/zvols, the installer's mirror ESP/boot copies and the swap, so
#   GNOME Files / Thunar (udisks2+gvfs) never list rpool, boot2, ESP2 ... as removable drives (observed on a desktop install). Init-neutral: plain udev rules.
# /usr/sbin/horcrux-admin-groups (-r6): every administrator account (wheel / horcrux-admin / patronus-admin member, UID >= 1000) is in the portage group, so
#   equery and eix work without sudo. The installers create the first administrator that way; pkg_postinst fixes existing installs on their next update, and
#   /etc/portage/postsync.d/60-horcrux-admin-groups re-checks after every tree sync (daily via horcrux-autoupdate), so an admin added later by hand is
#   covered too. gentoolkit (equery) and eix are part of core for the same reason; pkg_postinst builds the eix cache when it is missing.
# /etc/rc.conf.d/horcrux-host.conf (-r7): rc_sys="" so OpenRC never decides a Horcrux host is a container. An empty /.dockerenv from the image build
#   made OpenRC report "docker" and skip devfs, udev, udev-trigger, root (rw remount), localmount, fsck, hostname, modules: read-only root, no NIC driver,
#   only `lo`. The images wrote rc_sys="" into /etc/rc.conf, which sys-apps/openrc owns; accepting the ._cfg0000_rc.conf of an OpenRC update dropped it
#   (the diff `rc_sys=""` -> `#rc_sys=""` reads like a comment change). pkg_postinst also deletes a leftover container marker on a booted host.
# /etc/kernel/postinst.d + postrm.d/91-horcrux-boot-entry (-r7): installs without boot environments (XFS/ext4/LVM) had no hook that refreshes grub.cfg
#   (installkernel is built -grub because of the BE installs), so a kernel update was installed and never booted. BE installs keep 90-horcrux-be.
# /usr/sbin/horcrux-doctor + /etc/portage/postsync.d/60-horcrux-doctor (-r8): `horcrux-doctor portage-config` warns when a file or directory under
#   /etc/portage cannot be read by members of the portage group (a hand-placed package.use written under umask 077 made `emerge -p` fail for an
#   administrator with "Permission denied"). It judges access the way the kernel does (every directory searchable, the file's own group class decides,
#   symlinks followed), skips secrets (gnupg, auth.conf, key/token-like names), and `--fix` only adds read/search bits to root-owned entries inside
#   /etc/portage. The sync hook records the result in /var/lib/horcrux/doctor/portage-config.json. Init-neutral (plain script, no service).
RDEPEND="
	acct-group/portage
	app-portage/eix
	app-portage/gentoolkit
	sys-apps/shadow
	sys-auth/horcrux-ssh-pam
	net-misc/chrony
	net-misc/horcrux-time
	net-misc/horcrux-chrony-nm
	sys-apps/horcrux-firstboot
	sys-apps/horcrux-repo-config
	sys-apps/horcrux-backup
	sys-fs/zfs
	sys-kernel/dracut
	sys-kernel/installkernel
	dev-util/pahole
	sys-apps/horcrux-release
	sys-apps/horcrux-autoupdate
"

src_install() {
	insinto /etc/portage/package.use
	newins "${FILESDIR}"/package.use horcrux-core
	insinto /etc/docker
	newins "${FILESDIR}"/docker-daemon.json daemon.json
	insinto /etc/rc.conf.d
	newins "${FILESDIR}"/containerd-rc.conf horcrux-containerd.conf
	newins "${FILESDIR}"/host-rc.conf horcrux-host.conf
	exeinto /etc/kernel/postinst.d
	newexe "${FILESDIR}"/horcrux-boot-entry 91-horcrux-boot-entry
	exeinto /etc/kernel/postrm.d
	newexe "${FILESDIR}"/horcrux-boot-entry 91-horcrux-boot-entry
	insinto /etc/conf.d
	newins "${FILESDIR}"/horcrux-boot-entry.confd horcrux-boot-entry
	insinto /usr/lib/udev/rules.d
	doins "${FILESDIR}"/80-horcrux-udisks-hide.rules
	dosbin "${FILESDIR}"/horcrux-admin-groups
	dosbin "${FILESDIR}"/horcrux-doctor
	exeinto /etc/portage/postsync.d
	newexe "${FILESDIR}"/postsync-admin-groups 60-horcrux-admin-groups
	newexe "${FILESDIR}"/postsync-doctor 60-horcrux-doctor
	# marker: this is a Horcrux install (the installer, the QEMU gates and `horcrux-time-config` read it)
	insinto /usr/share/horcrux
	newins - core-release <<-EOF
		horcrux-core ${PVR}
	EOF
}

pkg_postinst() {
	elog "Horcrux core installed. Check the clock rules: horcrux-time-config check; enable at boot: rc-update add chronyd default; rc-update add horcrux-timesync default;"
	elog "rc-update add horcrux-firstboot default (the installer does all three)."
	udev_reload
	# a container marker on a BOOTED host is a leftover of the image build (`docker export` adds /.dockerenv); OpenRC and systemd both read it as "running in
	# a container". PID 1 being a real init tells a booted machine from the image-build container (whose PID 1 is a shell), where nothing is removed.
	if [[ -z ${ROOT} || ${ROOT} == / ]]; then
		local pid1 m
		pid1=$(readlink /proc/1/exe 2>/dev/null)
		case ${pid1##*/} in
			init|openrc-init|systemd)
				for m in /.dockerenv /run/.containerenv; do
					if [[ -f ${m} && ! -s ${m} ]]; then
						rm -f "${m}" && elog "removed the leftover container marker ${m} (OpenRC/systemd read it as 'running in a container')"
					fi
				done
				;;
		esac
	fi
	# re-evaluate the already-present disks now (no reboot, no session restart) so udisks drops the system storage at once
	if [[ -z ${ROOT} ]] && command -v udevadm >/dev/null 2>&1; then
		udevadm trigger --subsystem-match=block --action=change || true
	fi
	# administrators into the portage group (idempotent; also run after every tree sync)
	local line
	while IFS= read -r line; do
		[[ -n ${line} ]] && elog "horcrux-admin-groups: ${line}"
	done < <(ROOT="${EROOT}" "${EROOT%/}"/usr/sbin/horcrux-admin-groups 2>&1)
	# eix answers only from its cache: build it once so `eix` works right away (eix's own post-sync hook keeps it current). Only on a booted install
	# (firstboot done), never during an image build, whose tree is not shipped
	if [[ -z ${ROOT} ]] && [[ -e /var/lib/horcrux/firstboot-done ]] && [[ ! -s /var/cache/eix/portage.eix ]] && command -v eix-update >/dev/null 2>&1; then
		eix-update -q >/dev/null 2>&1 && elog "eix cache built (eix-update)" || ewarn "eix-update failed: run it once by hand"
	fi
}
