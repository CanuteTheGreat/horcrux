# Copyright 2025 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=""

inherit cargo systemd

DESCRIPTION="Proxmox VE alternative built natively for Gentoo"
HOMEPAGE="https://github.com/horcrux-project/horcrux"
SRC_URI="https://github.com/horcrux-project/${PN}/archive/v${PV}.tar.gz -> ${P}.tar.gz
	${CARGO_CRATE_URIS}"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"

# USE flags
# Virtualization backends (have Cargo features): qemu (default), lxd, incus
# Container runtimes (have Cargo features): lxc (default), docker (default), podman
# Features with Cargo support: backup, cluster
# Build options: cli, monitoring, systemd, webui (default)
# Note: Storage backends (zfs, ceph, etc) are runtime dependencies only - no Cargo features
IUSE="backup cli cluster docker incus lxc lxd +monitoring podman +qemu systemd +webui"

# At least one virtualization backend or container runtime must be enabled
REQUIRED_USE="|| ( qemu lxd incus lxc docker podman )"

# Runtime dependencies
# Note: All storage backends, networking, SSL, LDAP are always available at runtime
# These are suggested dependencies that can be installed as needed
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
	dev-libs/openssl:0=
	net-misc/bridge-utils
	sys-apps/iproute2
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

	# Build CLI if enabled
	if use cli; then
		einfo "Building CLI tool..."
		cargo_src_compile -p horcrux-cli
	fi

	# Build web UI if enabled
	if use webui; then
		einfo "Building web UI with trunk..."
		cd "${S}/horcrux-api/horcrux-ui" || die
		trunk build --release || die "Failed to build web UI"
	fi
}

src_install() {
	# Install API binary
	dobin target/release/horcrux-api

	# Install CLI if built
	if use cli; then
		dobin target/release/horcrux
	fi

	# Install web UI if built
	if use webui; then
		insinto /opt/horcrux
		doins -r horcrux-api/horcrux-ui/dist
		mv "${ED}/opt/horcrux/dist" "${ED}/opt/horcrux/horcrux-ui" || die
	fi

	# Install systemd services if enabled
	if use systemd; then
		systemd_dounit "${S}/deploy/systemd/horcrux.service"
		if use monitoring; then
			systemd_dounit "${S}/deploy/systemd/horcrux-monitoring.service"
		fi
	fi

	# Install OpenRC init scripts if not using systemd
	if ! use systemd; then
		newinitd "${S}/deploy/openrc/horcrux" horcrux
		if use monitoring; then
			newinitd "${S}/deploy/openrc/horcrux-monitoring" horcrux-monitoring
		fi
	fi

	# Create necessary directories
	keepdir /etc/horcrux
	keepdir /var/lib/horcrux/{vms,templates,cloudinit,backups}
	keepdir /var/log/horcrux
	keepdir /run/horcrux

	# Install default configuration
	insinto /etc/horcrux
	newins "${S}/deploy/config.toml.example" config.toml
	fperms 0640 /etc/horcrux/config.toml

	# Set permissions
	fowners root:root /var/lib/horcrux
	fperms 0700 /var/lib/horcrux

	dodoc README.md
}

pkg_postinst() {
	elog ""
	elog "Horcrux ${PV} - Gentoo Virtualization Platform"
	elog ""
	elog "Configuration: /etc/horcrux/config.toml"
	elog "Data directory: /var/lib/horcrux"
	elog ""

	if use systemd; then
		elog "Start service:"
		elog "  systemctl enable --now horcrux"
		if use monitoring; then
			elog "  systemctl enable --now horcrux-monitoring"
		fi
	else
		elog "Start service (OpenRC):"
		elog "  rc-update add horcrux default"
		elog "  rc-service horcrux start"
		if use monitoring; then
			elog "  rc-update add horcrux-monitoring default"
			elog "  rc-service horcrux-monitoring start"
		fi
	fi

	if use cli; then
		elog ""
		elog "CLI tool installed: horcrux"
		elog "  horcrux vm list              # List VMs"
		elog "  horcrux vm start <id>        # Start a VM"
		elog "  horcrux storage list         # List storage pools"
		elog "  horcrux cluster status       # Check cluster health"
		elog "  horcrux auth login           # Authenticate with API"
	fi

	if use webui; then
		elog ""
		elog "Web interface: https://$(hostname):8006"
	fi

	elog ""
	elog "Required kernel features:"
	if use qemu; then
		elog "  • KVM: CONFIG_KVM=m, CONFIG_KVM_INTEL=m (or CONFIG_KVM_AMD=m)"
	fi
	elog "  • Networking: CONFIG_TUN=m, CONFIG_BRIDGE=m, CONFIG_VXLAN=m, CONFIG_VLAN_8021Q=m"

	if use lxc; then
		elog "  • LXC: CONFIG_CGROUPS=y, CONFIG_NAMESPACES=y"
	fi

	elog ""
	elog "Optional storage backends (install as needed):"
	elog "  • ZFS:       emerge sys-fs/zfs"
	elog "  • Ceph:      emerge sys-cluster/ceph"
	elog "  • LVM:       emerge sys-fs/lvm2"
	elog "  • BtrFS:     emerge sys-fs/btrfs-progs"
	elog "  • GlusterFS: emerge sys-cluster/glusterfs"
	elog "  • CIFS/SMB:  emerge net-fs/cifs-utils"
	elog "  • NFS:       emerge net-fs/nfs-utils"
	elog "  • iSCSI:     emerge sys-block/open-iscsi"
	elog "  • LDAP:      emerge net-nds/openldap"
	elog ""
	elog "Storage backends are auto-detected at runtime."
	elog "S3 storage is built-in for backup/object storage."

	if use cluster; then
		elog ""
		elog "HA cluster support enabled"
		elog "Configure cluster members in /etc/horcrux/config.toml:[cluster]"
		elog "Requires: corosync and pacemaker"
	fi

	if use monitoring; then
		elog ""
		elog "Monitoring enabled - metrics available at http://localhost:9000/metrics"
	fi

	elog ""
}
