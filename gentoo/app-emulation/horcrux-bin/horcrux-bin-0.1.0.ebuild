# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit pam

DESCRIPTION="Horcrux manager (API server, web UI, CLI), prebuilt: the package every Horcrux edition installs so the node can join a cluster"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"

# WHY A -bin PACKAGE (lab finding 2026-11-05, "app-emulation/horcrux is masked ~amd64 and installed in no laptop edition"):
#  - app-emulation/horcrux-9999 / -0.1.0 build the whole workspace (cargo + wasm UI, ~1 h and several GB of RAM) with the network at build time:
#    not something a 4-8 GB laptop, a CI image layer or `emerge -uDN @world` can do, and it was never keyworded for a stable arch.
#  - The image build already cross-builds the manager on Debian bookworm with a portable CPU baseline (build/live-iso/project/Dockerfile.app, checked by
#    check-cpu-baseline.py) and leaves it as one tarball: /usr/share/horcrux/prebuilt/horcrux-manager-<arch>.tar.zst. THIS package installs that
#    tarball, so the manager is a real, owned, upgradable, removable Portage package (qlist -I app-emulation/horcrux-bin) in the server AND the desktop
#    editions (sys-apps/horcrux-core's image layer emerges it), with the runtime dependencies (qemu with SPICE, libvirt, bridge, dnsmasq) declared.
#  - A source build stays available: app-emulation/horcrux (blocked by this package, both install /usr/bin/horcrux-api).
# Upgrade on an installed machine: put a newer tarball in /usr/share/horcrux/prebuilt/ (or HORCRUX_MANAGER_TARBALL=/path) and `emerge -1 horcrux-bin`;
# without one the daily-image asset is fetched (HORCRUX_MANAGER_URL, default below).
RESTRICT="network-sandbox mirror strip"    # tarball fetch at src_unpack; the binaries are already stripped/cross built
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64 arm64 riscv"

RDEPEND="
	acct-group/kvm
	app-arch/zstd
	app-emulation/libvirt[qemu]
	app-emulation/qemu[spice,usbredir,virtfs,vnc]
	net-dns/dnsmasq
	net-misc/bridge-utils
	sys-apps/iproute2
	sys-auth/pambase
	sys-libs/pam
	virtual/libudev
!app-emulation/horcrux
"
BDEPEND="app-arch/zstd net-misc/curl"

S="${WORKDIR}/root"

_tarball_name() { echo "horcrux-manager-$(uname -m).tar.zst"; }

src_unpack() {
	local t="${HORCRUX_MANAGER_TARBALL:-${EPREFIX}/usr/share/horcrux/prebuilt/$(_tarball_name)}"
	mkdir -p "${S}" || die
	if [[ ! -s ${t} ]]; then
		local url="${HORCRUX_MANAGER_URL:-https://github.com/CanuteTheGreat/horcrux/releases/download/daily/horcrux-manager-daily-$(uname -m).tar.zst}"
		einfo "no local manager tarball (${t}): fetching ${url}"
		t="${T}/$(_tarball_name)"
		curl -fsSL --retry 3 -o "${t}" "${url}" || die "no manager tarball: put $(_tarball_name) in ${EPREFIX}/usr/share/horcrux/prebuilt/ or set HORCRUX_MANAGER_TARBALL"
	fi
	tar --zstd -xf "${t}" -C "${S}" --no-same-owner || die "unpacking ${t} failed"
	# sanity: a tarball of another layout must fail here, not leave a half manager
	[[ -x ${S}/usr/bin/horcrux-api && -x ${S}/usr/bin/horcrux && -d ${S}/usr/share/horcrux/ui ]] || die "${t} lacks usr/bin/horcrux-api, usr/bin/horcrux or usr/share/horcrux/ui"
}

src_install() {
	cp -a "${S}"/. "${ED}"/ || die
	# the prebuilt tree may carry the image's own init script; keep it executable
	[[ -e ${ED}/etc/init.d/horcrux-api ]] && fperms 0755 /etc/init.d/horcrux-api
	[[ -e ${ED}/etc/init.d/horcrux-guests ]] && fperms 0755 /etc/init.d/horcrux-guests
	keepdir /etc/horcrux/ssl /var/lib/horcrux /var/log/horcrux
	fperms 0700 /etc/horcrux/ssl
	# PAM realm (host accounts): the "horcrux" PAM service, same as the source ebuild
	pamd_mimic_system horcrux auth account
	insinto /etc/portage/package.use
	newins "${FILESDIR}"/package.use horcrux-manager
	# marker read by the lab (cluster-prereq) and the QEMU gates
	insinto /usr/share/horcrux
	newins - manager-release <<-EOF
		horcrux-bin ${PVR}
	EOF
}

pkg_postinst() {
	elog "Horcrux manager installed. It is NOT started by default on a desktop edition (the server profile enables it):"
	elog "  rc-update add libvirtd default; rc-update add horcrux-api default; rc-service horcrux-api start    # web UI https://<host>:8006"
	elog "Cluster: the first admin password is in /var/lib/horcrux/initial-admin-password (or /etc/horcrux/admin-password from the installer)."
}
