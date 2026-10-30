# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="HOGSMEADE (Alienware 18 Area-51) tools: installer front door, hardware profile, first-boot verifier, snapshots, backup, health, remote handoff"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"

RDEPEND="
	app-backup/restic
	app-shells/bash
	x11-libs/libnotify
	net-vpn/tailscale
	sys-apps/nvme-cli
	net-misc/rsync
	sys-apps/smartmontools
	sys-fs/cryptsetup
	sys-fs/zfs
	sys-power/switcheroo-control
	sys-process/cronie
	x11-apps/mesa-progs
"

src_install() {
	dobin "${FILESDIR}"/{prime-run,hogsmeade-verify,hogsmeade-install,hogsmeade-remote}
	dosbin "${FILESDIR}"/{hogsmeade-health,hogsmeade-snapshots,hogsmeade-backup}
	newinitd "${FILESDIR}/hogsmeade-firstboot.initd" hogsmeade-firstboot
	insinto /usr/share/live-installer/hwprofiles
	doins "${FILESDIR}/hogsmeade.toml"
	insinto /etc/modprobe.d
	newins "${FILESDIR}/nvidia-hogsmeade.conf" nvidia-hogsmeade.conf
	newins "${FILESDIR}/zfs-hogsmeade.conf" zfs-hogsmeade.conf
	insinto /etc/cron.d
	newins "${FILESDIR}/hogsmeade.cron" hogsmeade
	insinto /usr/share/hogsmeade
	doins "${FILESDIR}/POST-INSTALL.md"
	# early KMS: the Arrow Lake iGPU (8086:7D67, binds i915 or xe) must be up in the initramfs or the passphrase prompt is typed blind
	# (hogshead's first real boot). NVIDIA stays out of the initramfs on purpose (hybrid: the iGPU drives the panel).
	insinto /etc/dracut.conf.d
	newins - 93-hogsmeade-early-kms.conf <<-'EOF'
		# HOGSMEADE: Intel iGPU in the initramfs so the passphrase prompt is visible; firmware comes in through the modules' modinfo
		add_drivers+=" i915 xe drm_kms_helper drm_display_helper drm_buddy drm_exec drm_gpuvm gpu_sched ttm video "
		force_drivers+=" i915 xe "
		omit_drivers+=" nvidia nvidia_modeset nvidia_uvm nvidia_drm nouveau "
	EOF
	insinto /etc/hogsmeade
	newins - README <<-'EOF'
		backup.env   RESTIC_REPOSITORY / AWS_* / RESTIC_PASSWORD_FILE for hogsmeade-backup (installed by the private package)
		ts-authkey   one-time Tailscale handoff key (private package)
	EOF
}
