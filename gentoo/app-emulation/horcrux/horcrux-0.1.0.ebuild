# Copyright 2025 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=""

inherit cargo systemd

DESCRIPTION="Gentoo-based virtualization management platform"
HOMEPAGE="https://github.com/yourusername/horcrux"
SRC_URI="https://github.com/yourusername/${PN}/archive/v${PV}.tar.gz -> ${P}.tar.gz
	${CARGO_CRATE_URIS}"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS="~amd64"

# USE flags - leveraging Gentoo global flags where possible
# Virtualization: qemu (default), lxd, incus
# Containers: lxc (default), docker (default), podman
# Storage: zfs, ceph, lvm
# Features: backup, cluster, ssl, ldap
IUSE="backup ceph cluster docker incus ipv6 ldap lvm lxc lxd podman +qemu ssl systemd +webui zfs"

# At least one virtualization backend or container runtime must be enabled
REQUIRED_USE="|| ( qemu lxd incus lxc docker podman )"

# Runtime dependencies
RDEPEND="
	qemu? ( app-emulation/qemu[spice,usbredir,virtfs] )
	lxc? ( app-emulation/lxc )
	lxd? ( app-containers/lxd )
	incus? ( app-containers/incus )
	docker? ( app-containers/docker )
	podman? ( app-containers/podman )
	cluster? (
		sys-cluster/corosync
		sys-cluster/pacemaker
	)
	zfs? ( sys-fs/zfs )
	ceph? ( sys-cluster/ceph )
	lvm? ( sys-fs/lvm2 )
	ldap? ( net-nds/openldap )
	ssl? ( dev-libs/openssl:0= )
"

# Build dependencies
DEPEND="
	${RDEPEND}
	>=virtual/rust-1.82
"

BDEPEND="
	webui? ( dev-util/trunk )
"

# Cargo features mapping to USE flags
src_configure() {
	local myfeatures=(
		# Virtualization backends
		$(usex qemu "qemu" "")
		$(usex lxd "lxd" "")
		$(usex incus "incus" "")
		# Container runtimes
		$(usex lxc "lxc" "")
		$(usex docker "docker" "")
		$(usex podman "podman" "")
		# Additional features
		$(usex cluster "cluster" "")
		$(usex backup "backup" "")
	)

	cargo_src_configure --no-default-features
}

src_compile() {
	# Build API backend
	cargo_src_compile -p horcrux-api

	# Build web UI if enabled
	if use webui; then
		einfo "Building web UI with trunk..."
		cd "${S}/horcrux-ui" || die
		trunk build --release || die "Failed to build web UI"
	fi
}

src_install() {
	# Install API binary
	dobin target/release/horcrux-api

	# Install web UI if built
	if use webui; then
		insinto /usr/share/horcrux/www
		doins -r horcrux-ui/dist/*
	fi

	# Install systemd service if enabled
	if use systemd; then
		systemd_dounit "${FILESDIR}/horcrux-api.service"
	fi

	# Install OpenRC init script if not using systemd
	if ! use systemd; then
		newinitd "${FILESDIR}/horcrux-api.initd" horcrux-api
		newconfd "${FILESDIR}/horcrux-api.confd" horcrux-api
	fi

	# Create necessary directories
	keepdir /etc/horcrux
	keepdir /var/lib/horcrux
	keepdir /var/log/horcrux

	# Install default configuration
	insinto /etc/horcrux
	newins "${FILESDIR}/horcrux.conf" horcrux.conf.example

	dodoc README.md
}

pkg_postinst() {
	elog "Horcrux has been installed successfully!"
	elog ""
	elog "To start using Horcrux:"
	elog "  1. Copy /etc/horcrux/horcrux.conf.example to /etc/horcrux/horcrux.conf"
	elog "  2. Edit the configuration to suit your needs"
	if use systemd; then
		elog "  3. Enable and start the service: systemctl enable --now horcrux-api"
	else
		elog "  3. Enable and start the service: rc-update add horcrux-api default && rc-service horcrux-api start"
	fi
	if use webui; then
		elog ""
		elog "Web UI will be available at https://localhost:8006"
	fi
	elog ""
	elog "Make sure the following kernel features are enabled:"
	elog "  - KVM support (CONFIG_KVM, CONFIG_KVM_INTEL or CONFIG_KVM_AMD)"
	elog "  - Virtual networking (CONFIG_TUN, CONFIG_BRIDGE)"
	if use lxc; then
		elog "  - LXC support (CONFIG_CGROUPS, CONFIG_NAMESPACES)"
	fi
	if use zfs; then
		elog ""
		elog "ZFS support enabled. Make sure ZFS kernel module is loaded."
	fi
	if use cluster; then
		elog ""
		elog "Cluster support enabled. Configure Corosync in /etc/corosync/corosync.conf"
	fi
}
