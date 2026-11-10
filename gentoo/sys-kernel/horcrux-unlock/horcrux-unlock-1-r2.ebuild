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
	# early KMS: without i915 in the initramfs the panel stays on the GRUB framebuffer (or black) until the root
	# is mounted, so the passphrase prompt is typed blind. force_drivers loads it before udev settles; the
	# firmware (DMC/GuC/HuC) is pulled in through the module's modinfo.
	newins - 92-horcrux-early-kms.conf <<-'EOF'
		# Horcrux: Intel graphics (T2 Macs) in the initramfs so the passphrase prompt is visible
		add_drivers+=" i915 drm_kms_helper drm_display_helper drm_buddy ttm video "
		force_drivers+=" i915 "
	EOF
	# ZFS root + dist-kernel: the kernel update must rebuild the ZFS module IN THE SAME TRANSACTION and before the new initramfs is made. Without USE=dist-kernel
	# on zfs-kmod nothing rebuilds it for the new kernel: dracut then fails with "Failed to find module zfs" (or the initramfs has no ZFS) and the ZFS-root laptop
	# does not boot (gringotts, 2026-11-03: shipped without the flag). Kernel builds that are not dist-kernel (the T2 variant) override it in their own package.use file.
	insinto /etc/portage/package.use
	newins - horcrux-zfs <<-'EOF'
		# owned by sys-kernel/horcrux-unlock: rebuild the ZFS kernel module with every dist-kernel update (see the package ebuild)
		sys-fs/zfs rootfs dist-kernel
		sys-fs/zfs-kmod dist-kernel
	EOF
}
