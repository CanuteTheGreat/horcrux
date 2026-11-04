# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="dracut module: one passphrase for LUKS2 hibernation swap and native-encrypted ZFS root, resume before pool import"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
# USE=-encrypted is the TEST-MACHINE mode (owner decision 2026-11-04: gringotts, hogshead, madammalkins while they are test machines):
# no unlock module, no early-KMS for a passphrase prompt, no cryptsetup in the initramfs: the pool/root is opened unattended and only the
# stock dracut `resume` + `zfs` modules are configured (resume still runs before the pool import). Set it through
# app-misc/horcrux-test-machine, never by hand. Turn encryption back on: see that package's README (reinstall with the installer's
# encryption choice; native ZFS encryption cannot be added or removed in place).
IUSE="+encrypted"

RDEPEND="
	sys-kernel/dracut
	sys-fs/zfs
	encrypted? ( sys-fs/cryptsetup )
"

src_install() {
	insinto /etc/dracut.conf.d
	if use encrypted; then
		exeinto /usr/lib/dracut/modules.d/91horcrux-unlock
		doexe "${FILESDIR}"/*.sh
		newins - 91-horcrux-unlock.conf <<-'EOF2'
			# Horcrux: LUKS2 hibernation swap + ZFS with ONE passphrase (dracut module horcrux-unlock)
			add_dracutmodules+=" crypt dm resume horcrux-unlock zfs "
			add_drivers+=" dm_crypt "
		EOF2
		# early KMS: without i915 in the initramfs the panel stays on the GRUB framebuffer (or black) until the root
		# is mounted, so the passphrase prompt is typed blind.
		newins - 92-horcrux-early-kms.conf <<-'EOF2'
			# Horcrux: Intel graphics (T2 Macs) in the initramfs so the passphrase prompt is visible
			add_drivers+=" i915 drm_kms_helper drm_display_helper drm_buddy ttm video "
			force_drivers+=" i915 "
		EOF2
	else
		newins - 91-horcrux-unlock.conf <<-'EOF2'
			# Horcrux TEST MACHINE (horcrux-unlock USE=-encrypted): NO disk encryption, no passphrase prompt, unattended boot.
			# Hibernation resume (plain swap partition) still runs before the pool import via the stock resume module.
			add_dracutmodules+=" resume zfs "
			omit_dracutmodules+=" crypt dm horcrux-unlock "
		EOF2
	fi
	# ZFS root + dist-kernel: the kernel update must rebuild the ZFS module IN THE SAME TRANSACTION and before the new initramfs is made
	# (without USE=dist-kernel on zfs-kmod dracut fails with "Failed to find module zfs" and the ZFS-root laptop does not boot).
	insinto /etc/portage/package.use
	newins - horcrux-zfs <<-'EOF2'
		# owned by sys-kernel/horcrux-unlock: rebuild the ZFS kernel module with every dist-kernel update (see the package ebuild)
		sys-fs/zfs rootfs dist-kernel
		sys-fs/zfs-kmod dist-kernel
	EOF2
}
