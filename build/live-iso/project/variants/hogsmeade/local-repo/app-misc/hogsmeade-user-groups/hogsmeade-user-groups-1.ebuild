# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): the owner's account belongs to every
# group he needs (docker, serial, adm, portage, seat, comfyui, qemu, tss, ... and the web UI admin group). Owner rule: package-managed, never
# `usermod` by hand. The list lives in ONE file, /usr/share/horcrux/desktop/extra-groups.list, which the installer's user creation
# (liveinst/desktop.py create_user) reads when it creates the account on a fresh install, and pkg_postinst applies to an installed machine
# (idempotent; groups that do not exist are skipped; a missing user is not an error: the installer adds him later).
# Groups that come from packages are pulled in as acct-group/* dependencies. cdrw and wheel have no acct-group package (created by the image).
EAPI=8

DESCRIPTION="HOGSMEADE: package-owned group memberships of the owner's account"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	acct-group/adm
	acct-group/comfyui
	acct-group/dialout
	acct-group/docker
	acct-group/horcrux-admin
	acct-group/libvirt
	acct-group/netdev
	acct-group/pcap
	acct-group/pcscd
	acct-group/portage
	acct-group/qemu
	acct-group/seat
	acct-group/tss
	acct-group/uucp
	sys-apps/shadow
"

src_install() {
	insinto /usr/share/horcrux/desktop
	newins - extra-groups.list <<-'EOF2'
		# groups of the HOGSMEADE owner's account (one per line), see app-misc/hogsmeade-user-groups
		user canutethegreat
		docker
		dialout
		uucp
		adm
		portage
		cdrw
		netdev
		seat
		comfyui
		qemu
		tss
		horcrux-admin
		libvirt
		pcap
		pcscd
		wheel
		audio
		video
		usb
		input
		users
		kvm
		render
		lp
		lpadmin
		cdrom
		pipewire
		plugdev
	EOF2
}

pkg_postinst() {
	local list="${EROOT}/usr/share/horcrux/desktop/extra-groups.list" u g have
	u=$(sed -n 's/^user //p' "${list}")
	getent passwd "${u}" >/dev/null || { einfo "user ${u} does not exist yet: the installer adds the groups"; return 0; }
	have=" $(id -nG "${u}") "
	while read -r g; do
		case "${g}" in ""|"#"*|"user "*) continue ;; esac
		[[ ${have} == *" ${g} "* ]] && continue
		getent group "${g}" >/dev/null || { ewarn "group ${g} does not exist, skipped"; continue; }
		usermod -aG "${g}" "${u}" || ewarn "could not add ${u} to ${g}"
		einfo "added ${u} to ${g}"
	done < "${list}"
}
