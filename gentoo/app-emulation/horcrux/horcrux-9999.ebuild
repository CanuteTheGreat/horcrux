# Copyright 2025 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=""

inherit cargo git-r3 optfeature pam systemd

DESCRIPTION="Proxmox VE alternative built natively for Gentoo"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"

EGIT_REPO_URI="https://github.com/CanuteTheGreat/horcrux.git"

# CARGO_CRATE_URIS is populated at emerge time for live ebuilds via
# `cargo vendor` in the build container (see repo Dockerfile) since crate
# hashes cannot be pinned ahead of time when tracking HEAD.

RESTRICT="network-sandbox"  # webui USE fetches the prebuilt trunk bundler at build time (no ebuild exists)
LICENSE="GPL-2"
SLOT="0"
# Live ebuilds are not keyworded (see Gentoo devmanual, live ebuilds section)

# USE flags
# Every flag below except the build options maps 1:1 to a horcrux-api Cargo
# feature (src_configure) that gates real code: a disabled backend is not
# compiled in, its API routes answer 404 and the web UI hides it (via
# GET /api/features). gentoo/ci/use-matrix.sh proves this in CI.
# Virtualization backends: qemu (default), lxd, incus
# Container runtimes: lxc, docker, podman
# Platform features: backup (default), cluster (+HA), kubernetes, nas
# Build options: cli, monitoring, systemd, webui (default)
# NAS protocols: smb, nfs-server, afp, webdav, ftp
# NAS auth: ldap (client), ldap-server, kerberos, ad (Active Directory)
# NAS services: timemachine, s3-gateway, iscsi-target, rsync-server
# NAS storage: nas-zfs, nas-btrfs, nas-mdraid, nas-lvm
IUSE="
	+backup cli cluster docker incus kubernetes ldap lxc lxd +monitoring +pam podman +qemu systemd +webui
	nas smb nfs-server afp webdav ftp
	ldap-server kerberos ad
	timemachine s3-gateway iscsi-target rsync-server
	nas-zfs nas-btrfs nas-mdraid nas-lvm
"

# At least one virtualization backend or container runtime must be enabled
# NAS protocol dependencies
REQUIRED_USE="
	|| ( qemu lxd incus lxc docker podman nas )
	smb? ( nas )
	nfs-server? ( nas )
	afp? ( nas )
	webdav? ( nas )
	ftp? ( nas )
	ldap-server? ( nas )
	kerberos? ( nas )
	ldap? ( nas )
	ad? ( nas kerberos )
	timemachine? ( afp )
	s3-gateway? ( nas )
	iscsi-target? ( nas )
	rsync-server? ( nas )
	nas-zfs? ( nas )
	nas-btrfs? ( nas )
	nas-mdraid? ( nas )
	nas-lvm? ( nas )
"

# Runtime dependencies
# Note: All storage backends, networking, SSL, LDAP are always available at runtime
# These are suggested dependencies that can be installed as needed
# NOTE: no "#" comments inside dependency strings -- PMS parses them as
# atoms ("Invalid atom (#)") and the whole ebuild fails metadata.
# kubernetes? pulls app-admin/helm: Helm repositories/charts/releases are
# driven through the helm CLI.
RDEPEND="
	qemu? (
		app-emulation/qemu[spice,usbredir,virtfs,vnc]
		app-emulation/libvirt
	)
	lxc? (
		app-containers/lxc
		sys-apps/shadow
		sys-fs/e2fsprogs
	)
	lxd? ( app-containers/lxd )
	incus? ( app-containers/incus )
	docker? ( app-containers/docker )
	podman? ( app-containers/podman )
	backup? (
		app-arch/tar
		app-arch/zstd
		app-arch/gzip
		app-arch/lzop
	)
	cluster? (
		sys-cluster/corosync
		sys-cluster/pacemaker
	)
	kubernetes? ( app-admin/helm )
	dev-libs/openssl:0=
	pam? ( sys-libs/pam )
	net-misc/bridge-utils
	sys-apps/iproute2

	smb? ( net-fs/samba[acl,winbind] )
	nfs-server? ( net-fs/nfs-utils[nfsv4] )
	afp? ( net-fs/netatalk )
	webdav? ( www-servers/nginx[dav] )
	ftp? ( net-ftp/proftpd[ssl] )

	ldap-server? ( net-nds/openldap[slapd] )
	kerberos? ( app-crypt/mit-krb5 )
	ad? (
		net-fs/samba[ads,winbind]
		app-crypt/mit-krb5
	)

	timemachine? ( net-fs/netatalk )
	s3-gateway? (
		app-misc/silo-bin
		app-misc/mcli-bin
	)
	iscsi-target? ( sys-block/tgt )
	rsync-server? ( net-misc/rsync )

	nas-zfs? ( sys-fs/zfs )
	nas-btrfs? ( sys-fs/btrfs-progs )
	nas-mdraid? ( sys-fs/mdadm )
	nas-lvm? ( sys-fs/lvm2 )

	nas? ( sys-apps/acl )
"

# Build dependencies
# NOTE: there is no virtual/rust package in the Gentoo tree (never has been -
# only dev-lang/rust and dev-lang/rust-bin are real ebuilds); depend on either
# provider directly instead of a nonexistent virtual. Mirrors the fix already
# applied to horcrux-0.1.0.ebuild - this live ebuild had been missed.
DEPEND="
	${RDEPEND}
	|| ( >=dev-lang/rust-1.82:= >=dev-lang/rust-bin-1.82:= )
"

BDEPEND=""

# NOTE: both the cargo and git-r3 eclasses export src_unpack; since git-r3 is
# inherited after cargo, its src_unpack wrapper silently replaces cargo's,
# so cargo_gen_config() never runs and any later cargo_env/cargo_src_compile
# call dies with: "FATAL: please call cargo_gen_config before using cargo_env"
# Define src_unpack explicitly to run both (live ebuild: use cargo_live_src_unpack).
src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

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
		$(usex pam "pam" "")
		$(usex backup "backup" "")
		$(usex kubernetes "kubernetes" "")

		# NAS Core
		$(usex nas "nas" "")

		# NAS File Sharing Protocols
		$(usex smb "smb" "")
		$(usex nfs-server "nfs-server" "")
		$(usex afp "afp" "")
		$(usex webdav "webdav" "")
		$(usex ftp "ftp" "")

		# NAS Authentication
		$(usex ldap-server "ldap-server" "")
		$(usex ldap "ldap" "")
		$(usex kerberos "kerberos" "")
		$(usex ad "ad" "")

		# NAS Services
		$(usex timemachine "timemachine" "")
		$(usex s3-gateway "s3-gateway" "")
		$(usex iscsi-target "iscsi-target" "")
		$(usex rsync-server "rsync-server" "")

		# NAS Storage Backends
		$(usex nas-zfs "nas-zfs" "")
		$(usex nas-btrfs "nas-btrfs" "")
		$(usex nas-mdraid "nas-mdraid" "")
		$(usex nas-lvm "nas-lvm" "")
	)

	cargo_src_configure --no-default-features
}

src_compile() {
	# Build API backend
	cargo_src_compile -p horcrux-api

	# Build CLI if enabled
	if use cli; then
		einfo "Building CLI tool..."
		# NOTE: cargo_src_compile would reuse the workspace-wide
		# ECARGO_ARGS set in src_configure() above (e.g. --features
		# docker/qemu/lxc/...), but horcrux-cli's own Cargo.toml
		# defines none of those Cargo features -- it's a thin API
		# client with its own independent dependency set. Cargo
		# rejects unknown --features flags per-package even inside a
		# workspace build ("does not contain this feature"), so this
		# broke the build the moment more than one virtualization/
		# container-runtime USE flag was enabled together. Build the
		# CLI package directly via cargo_env, bypassing ECARGO_ARGS,
		# since it has no USE-gated Cargo features to select.
		set -- "${CARGO}" build $(usex debug "" --release) -p horcrux-cli
		einfo "${@}"
		cargo_env "${@}" || die "cargo build failed (horcrux-cli)"
	fi

	# Build web UI if enabled
	if use webui; then
		# trunk (the Leptos WASM bundler) has no Gentoo ebuild anywhere in
		# the tree. `cargo install trunk` cannot work here: cargo.eclass
		# points CARGO_HOME at the offline vendored registry, which has no
		# trunk ("could not find `trunk` in registry `crates-io`"), and a
		# from-source build also hits trunk's lightningcss/cssparser
		# conflict. Use the upstream prebuilt release instead (the same
		# version CI's Build Web UI job currently resolves), unpacked into ${T};
		# RESTRICT=network-sandbox already allows the fetch.
		local trunk
		if command -v trunk >/dev/null 2>&1; then
			trunk=$(command -v trunk)
		else
			local trunk_ver=0.21.14 trunk_arch
			case ${ARCH} in
				amd64) trunk_arch=x86_64-unknown-linux-gnu ;;
				arm64) trunk_arch=aarch64-unknown-linux-gnu ;;
				*) die "USE=webui: no prebuilt trunk for ${ARCH}; install trunk into PATH first" ;;
			esac
			local trunk_url="https://github.com/trunk-rs/trunk/releases/download/v${trunk_ver}/trunk-${trunk_arch}.tar.gz"
			einfo "Fetching prebuilt trunk ${trunk_ver} (${trunk_arch})..."
			mkdir -p "${T}/trunk" || die
			if command -v curl >/dev/null 2>&1; then
				curl -fsSL "${trunk_url}" -o "${T}/trunk.tar.gz"
			else
				wget -q -O "${T}/trunk.tar.gz" "${trunk_url}"
			fi || die "Failed to download trunk from ${trunk_url}"
			tar -xzf "${T}/trunk.tar.gz" -C "${T}/trunk" || die "Failed to unpack trunk"
			trunk="${T}/trunk/trunk"
		fi
		# The UI is compiled for wasm32-unknown-unknown, and dev-lang/rust-bin
		# ships only the host's standard library (it has no wasm USE flag):
		# the build died with "can't find crate for `core`". Unless the
		# installed toolchain already has the wasm32 target, fetch the
		# official rust-std component for exactly this rustc version and
		# build the UI against a sysroot made of it plus links to the
		# toolchain's own target dirs (rust-lld lives there).
		local wasm_rustflags=
		local sysroot
		sysroot=$(rustc --print sysroot) || die
		if [[ ! -d ${sysroot}/lib/rustlib/wasm32-unknown-unknown ]]; then
			local rustver std_url wasm_sysroot d
			rustver=$(rustc --version | awk '{print $2}') || die
			std_url="https://static.rust-lang.org/dist/rust-std-${rustver}-wasm32-unknown-unknown.tar.xz"
			einfo "Fetching the wasm32 standard library for rustc ${rustver}..."
			if command -v curl >/dev/null 2>&1; then
				curl -fsSL "${std_url}" -o "${T}/rust-std-wasm32.tar.xz"
			else
				wget -q -O "${T}/rust-std-wasm32.tar.xz" "${std_url}"
			fi || die "Failed to download ${std_url}"
			tar -xJf "${T}/rust-std-wasm32.tar.xz" -C "${T}" \
				|| die "Failed to unpack the wasm32 standard library"
			wasm_sysroot="${T}/rust-std-${rustver}-wasm32-unknown-unknown/rust-std-wasm32-unknown-unknown"
			for d in "${sysroot}"/lib/rustlib/*; do
				[[ -e ${wasm_sysroot}/lib/rustlib/${d##*/} ]] \
					|| ln -s "${d}" "${wasm_sysroot}/lib/rustlib/${d##*/}" || die
			done
			wasm_rustflags="--sysroot=${wasm_sysroot}"
		fi
		einfo "Building web UI with trunk..."
		cd "${S}/horcrux-api/horcrux-ui" || die
		# trunk builds with an explicit --target, so RUSTFLAGS only reach
		# the wasm crates, never host build scripts or proc-macros.
		RUSTFLAGS="${wasm_rustflags}" "${trunk}" build --release || die "Failed to build web UI"
	fi
}

src_install() {
	# Install API binary
	# cargo_target_dir (cargo.eclass) honours USE=debug and cross targets;
	# a hardcoded target/release broke `USE=debug` installs.
	dobin "$(cargo_target_dir)"/horcrux-api

	# Install CLI if built
	if use cli; then
		dobin "$(cargo_target_dir)"/horcrux
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
		systemd_dounit "${S}/deploy/systemd/horcrux-guests.service"
		if use monitoring; then
			systemd_dounit "${S}/deploy/systemd/horcrux-monitoring.service"
		fi
	fi

	# Install OpenRC init scripts if not using systemd
	if ! use systemd; then
		newinitd "${S}/deploy/openrc/horcrux" horcrux
		newinitd "${S}/deploy/openrc/horcrux-guests" horcrux-guests
		if use monitoring; then
			newinitd "${S}/deploy/openrc/horcrux-monitoring" horcrux-monitoring
		fi
	fi

	# Create necessary directories
	keepdir /etc/horcrux
	# HTTPS certificate + node-local CA are generated here on first start
	keepdir /etc/horcrux/ssl
	fperms 0700 /etc/horcrux/ssl

	# PAM realm (host accounts): the "horcrux" PAM service
	if use pam; then
		pamd_mimic_system horcrux auth account
	fi
	keepdir /var/lib/horcrux/{vms,templates,cloudinit,backups}
	keepdir /var/log/horcrux
	keepdir /run/horcrux

	# Create NAS directories if NAS is enabled
	if use nas; then
		keepdir /var/lib/horcrux/nas/{shares,users,pools}
		keepdir /var/lib/horcrux/nas/snapshots
		keepdir /var/lib/horcrux/nas/replication
	fi
	if use s3-gateway; then
		keepdir /var/lib/horcrux/nas/s3
	fi
	if use iscsi-target; then
		keepdir /var/lib/horcrux/nas/iscsi
	fi

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
	if use qemu; then
		optfeature "UEFI (OVMF) firmware for QEMU VMs" sys-firmware/edk2-bin
		optfeature "emulated TPM 2.0 for QEMU VMs" app-crypt/swtpm
		optfeature "saving the state of running containers (Guest Power: CRIU stateful stop)" sys-process/criu
	fi
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
		elog "  • LXC: CONFIG_CGROUPS=y, CONFIG_NAMESPACES=y, CONFIG_USER_NS=y, CONFIG_VETH=m,"
		elog "    CONFIG_BRIDGE_VLAN_FILTERING=y (VLAN tags on container NICs), CONFIG_BLK_DEV_LOOP=m (sized disks)"
		elog "  • Unprivileged LXC containers map root to host uid 100000+; Horcrux adds"
		elog "    root:100000:65536 to /etc/subuid and /etc/subgid when it creates one."
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

	if use nas; then
		elog ""
		elog "╔══════════════════════════════════════════════════════════════════╗"
		elog "║               NAS (Network Attached Storage) Enabled             ║"
		elog "╚══════════════════════════════════════════════════════════════════╝"
		elog ""
		elog "NAS configuration: /etc/horcrux/config.toml:[nas]"
		elog "NAS data directory: /var/lib/horcrux/nas"
		elog ""
		elog "Enabled protocols:"
		if use smb; then
			elog "  • SMB/CIFS (Samba) - Windows file sharing"
			elog "    Config: /etc/samba/smb.conf (managed by horcrux)"
		fi
		if use nfs-server; then
			elog "  • NFS Server - Unix/Linux exports"
			elog "    Config: /etc/exports (managed by horcrux)"
		fi
		if use afp; then
			elog "  • AFP (Netatalk) - macOS file sharing"
			elog "    Config: /etc/netatalk/afp.conf (managed by horcrux)"
		fi
		if use webdav; then
			elog "  • WebDAV - Web-based file access"
		fi
		if use ftp; then
			elog "  • FTP/SFTP (ProFTPD)"
			elog "    Config: /etc/proftpd/proftpd.conf (managed by horcrux)"
		fi
		elog ""
		if use timemachine; then
			elog "Time Machine backup target enabled (via AFP)"
		fi
		if use s3-gateway; then
			elog "S3 Gateway (MinIO) enabled for object storage API"
		fi
		if use iscsi-target; then
			elog "iSCSI Target enabled for block storage"
		fi
		if use rsync-server; then
			elog "Rsync server enabled for efficient backups"
		fi
		if use ad; then
			elog ""
			elog "Active Directory integration enabled"
			elog "  Join domain: horcrux nas ad-join --domain EXAMPLE.COM"
		elif use kerberos; then
			elog ""
			elog "Kerberos authentication enabled"
		fi
		if use ldap-server; then
			elog ""
			elog "LDAP server (OpenLDAP) enabled for directory services"
		fi
		elog ""
		if use cli; then
			elog "NAS CLI commands:"
			elog "  horcrux nas share-list            # List shares"
			elog "  horcrux nas share-create          # Create a share"
			elog "  horcrux nas user-list             # List NAS users"
			elog "  horcrux nas pool-list             # List storage pools"
			elog "  horcrux nas snapshot-list         # List snapshots"
		fi
	fi

	elog ""
}
