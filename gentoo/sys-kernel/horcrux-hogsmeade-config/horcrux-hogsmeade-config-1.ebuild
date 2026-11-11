# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Kernel config + initramfs config for the Alienware 18 Area-51 (HOGSMEADE): VMD, SoundWire/SOF, rt722/rt1320, BE200, r8169, xe/i915, ISH, alienware-wmi"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

# deliberately no dependency on the kernel (it would pull an hour of compiling into every config update)
RDEPEND="sys-kernel/dracut"

src_install() {
	# merged by the dist-kernel eclass after Gentoo's own config
	insinto /etc/kernel/config.d
	newins "${FILESDIR}/hogsmeade.config" 90-horcrux-hogsmeade.config
	insinto /usr/share/horcrux-hogsmeade
	doins "${FILESDIR}/required-config.txt"

	# initramfs: keyboard + NVMe/VMD + dm-crypt are needed at the one passphrase prompt, with or without VMD
	insinto /etc/dracut.conf.d
	newins - 90-horcrux-hogsmeade.conf <<-'EOF'
		# HOGSMEADE: generic (not host-only) initramfs, boots with the BIOS in VMD or AHCI mode
		hostonly="no"
		add_drivers+=" vmd nvme nvme_core ahci xhci_pci xhci_hcd hid hid_generic usbhid atkbd i8042 dm_crypt dm_mod intel_lpss_pci i2c_hid_acpi hid_multitouch "
		add_dracutmodules+=" crypt dm "
		omit_dracutmodules+=" plymouth "
	EOF
}
