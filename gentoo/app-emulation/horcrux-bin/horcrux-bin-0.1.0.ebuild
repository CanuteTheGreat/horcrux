# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

inherit pam systemd

DESCRIPTION="Horcrux manager (API server, web UI, CLI), prebuilt: the package every Horcrux edition installs so the node can join a cluster"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"

# WHY A -bin PACKAGE (lab finding 2026-11-05, "app-emulation/horcrux is masked ~amd64 and installed in no laptop edition"):
#  - app-emulation/horcrux-9999 / -0.1.0 build the whole workspace (cargo + wasm UI, ~1 h and several GB of RAM) with the network at build time:
#    not something a 4-8 GB laptop, a CI image layer or `emerge -uDN @world` can do, and it was never keyworded for a stable arch.
#  - The image build already cross-builds the manager on Debian bookworm with a portable CPU baseline (build/live-iso/project/Dockerfile.app, checked by
#    check-cpu-baseline.py) and leaves it as one tarball: /usr/share/horcrux/prebuilt/horcrux-manager-<arch>.tar.zst. THIS package installs that
#    tarball, so the manager is a real, owned, upgradable, removable Portage package (qlist -I app-emulation/horcrux-bin) in the server AND the desktop
#    editions (sys-apps/horcrux-core's image layer emerges it), with the runtime dependencies (qemu with SPICE, libvirt, bridge, dnsmasq, nfs-utils for shared storage) declared.
#  - A source build stays available: app-emulation/horcrux (blocked by this package, both install /usr/bin/horcrux-api).
# Upgrade on an installed machine: put a newer tarball in /usr/share/horcrux/prebuilt/ (or HORCRUX_MANAGER_TARBALL=/path) and `emerge -1 horcrux-bin`;
# without one the daily-image asset is fetched (HORCRUX_MANAGER_URL, default below).
#
# STALE BINPKGS (lab 2026-11-10): this ebuild used to be ONE version (0.1.0) for every tarball. On a host with FEATURES=buildpkg and
# --usepkg/--getbinpkg (the lab laptops, any binhost user) the first install left a binary package 0.1.0 behind and every later
# `emerge -1 horcrux-bin` silently reinstalled THAT package, never reading the new tarball (the rolling upgrade 'installed' the old sha).
# Portage matches binary packages by CPV, so the version must change with the tarball: bump-horcrux-bin.sh (this directory; run by the
# bump-horcrux-bin workflow whenever the published daily manager tarball changes) copies this file to horcrux-bin-0.1.0_p<YYYYMMDDHHMM>.ebuild,
# a NEWER version for Portage: `emerge -u horcrux-bin` sees an upgrade, no binary package of it exists, so the tarball is fetched and
# installed. The plain 0.1.0 stays as the fallback for a tree without a stamped ebuild (image builds from a local tarball).
# The two HORCRUX_MANAGER_SHA256_* values are filled in by the bump script (empty here): a tarball FETCHED by URL must match the build
# this version was cut for (a replaced daily asset fails loudly instead of installing under the wrong version).
HORCRUX_MANAGER_SHA256_x86_64=""
HORCRUX_MANAGER_SHA256_aarch64=""
RESTRICT="network-sandbox mirror strip"    # tarball fetch at src_unpack; the binaries are already stripped/cross built
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64 arm64 riscv"

# qemu[zstd]: qcow2 images with zstd compression (libvirt / virt-manager default on some distros) are imported with qemu-img; without
# USE=zstd qemu-img answers "unknown compression type: 1" and the VM importer cannot read the disk (HOGSMEADE 2026-11-12).
RDEPEND="
	acct-group/horcrux-admin
	acct-group/kvm
	app-arch/zstd
	app-emulation/libvirt[qemu]
	app-emulation/qemu[spice,usbredir,virtfs,vnc,zstd]
	net-dns/dnsmasq
	net-fs/nfs-utils
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
		local want_var="HORCRUX_MANAGER_SHA256_$(uname -m)" want got
		want=${!want_var}
		if [[ -n ${want} && -z ${HORCRUX_MANAGER_URL} ]]; then
			got=$(sha256sum "${t}" | cut -d' ' -f1)
			if [[ ${got} != "${want}" ]]; then
				# The daily asset is republished on every main build, several times a day, and the overlay only learns of it at the next
				# bump + sync (up to ~2 h): a dying unpack made every `emerge -u @world` in that window fail (lab madammalkins j02219/j02103).
				# The republished tarball is always the newer build, so install it and say so; the real sha goes into the release marker,
				# and the next stamped version (cut for this sha) is still an upgrade for Portage.
				ewarn "the daily manager tarball was republished after ${PVR} was cut (sha256 ${got}, expected ${want}): installing the newer build; \`emaint sync -r horcrux\` brings the ebuild cut for it"
			fi
			HORCRUX_FETCHED_SHA256=${got}
		fi
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
	# The HA fence agent (`horcrux-api ha-fence-agent`) is its own boot service: it answers the cluster leader's GET /attest (:8026) and kills the
	# HA guests of a node whose manager hung, WITHOUT the manager (j01743/j02106: a node rebooted with its manager down was held 322 s). The package
	# always installs it from files/ (a tarball built before it existed has no copy), and the unit/script are the ones the manager itself
	# rewrites at start (ha_fence_agent.rs ensure_service), so the two never disagree (test_ha_fence_packaging).
	rm -f "${ED}"/etc/init.d/horcrux-ha-fence
	newinitd "${FILESDIR}"/horcrux-ha-fence.initd horcrux-ha-fence
	systemd_dounit "${FILESDIR}"/horcrux-ha-fence.service
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
		${HORCRUX_FETCHED_SHA256:+tarball-sha256 ${HORCRUX_FETCHED_SHA256}}
	EOF
}

pkg_postinst() {
	# Enable the fence agent where HA can be configured: a node whose manager starts at boot (server editions, every cluster member: the manager
	# is enabled on create/join) also gets its fence agent in the default runlevel. A desktop edition without the manager enabled stays as is.
	# (A symlink, not rc-update: this also works with ROOT != /; the manager's ensure_service does the same at runtime.)
	local rl=${EROOT}/etc/runlevels/default
	if [[ -L ${rl}/horcrux-api && ! -e ${rl}/horcrux-ha-fence ]]; then
		ln -s /etc/init.d/horcrux-ha-fence "${rl}/horcrux-ha-fence" && elog "horcrux-ha-fence enabled in the default runlevel (the manager is)"
	fi
	elog "Horcrux manager installed. It is NOT started by default on a desktop edition (the server profile enables it):"
	elog "  rc-update add libvirtd default; rc-update add horcrux-api default; rc-service horcrux-api start    # web UI https://<host>:8006"
	elog "Web UI login: sign in with a SYSTEM user (PAM service /etc/pam.d/horcrux) who is in the horcrux-admin or wheel group; there is no 'admin' account."
	elog "  gpasswd -a <user> horcrux-admin     # on every node: each node authenticates against its own users (or a shared SSSD/LDAP directory)"
	elog "  root cannot log in to the web UI unless [auth] allow_root = true in /etc/horcrux/config.toml; see docs/WEB-LOGIN-PAM.md"
	elog "MIGRATION: old Horcrux-only accounts keep working for this release (read-only); the legacy 'admin' is disabled by the first PAM administrator login."
}
