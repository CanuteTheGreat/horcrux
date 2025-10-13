# Copyright 2025 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2

EAPI=8

CRATES=""

inherit cargo systemd

DESCRIPTION="Horcrux - High-performance virtualization management platform for Gentoo"
HOMEPAGE="https://github.com/yourusername/horcrux"
SRC_URI="https://github.com/yourusername/horcrux/archive/v${PV}.tar.gz -> ${P}.tar.gz"

LICENSE="MIT Apache-2.0"
SLOT="0"
KEYWORDS="~amd64"
IUSE="qemu lxc docker podman cluster backup gpu"

RDEPEND="
	>=virtual/rust-1.82.0
	dev-db/sqlite:3
	qemu? (
		app-emulation/qemu[qemu_softmmu_targets_x86_64]
		app-emulation/qemu[virtfs]
		app-emulation/qemu[vhost-net]
	)
	lxc? ( app-emulation/lxc )
	docker? ( app-containers/docker )
	podman? ( app-containers/podman )
	cluster? (
		sys-cluster/corosync
		sys-cluster/pacemaker
	)
	backup? ( app-arch/zstd )
	gpu? ( x11-drivers/nvidia-drivers )
	sys-apps/systemd
	dev-libs/openssl:=
	sys-libs/pam
"

DEPEND="${RDEPEND}"
BDEPEND="
	>=virtual/rust-1.82.0
	dev-util/cargo
"

QA_FLAGS_IGNORED="usr/bin/horcrux.*"

src_unpack() {
	cargo_src_unpack
}

src_configure() {
	local myfeatures=(
		$(usev qemu)
		$(usev lxc)
		$(usev docker)
		$(usev podman)
		$(usev cluster)
		$(usev backup)
	)

	cargo_src_configure --no-default-features
}

src_compile() {
	# Build API server
	cargo_src_compile -p horcrux-api

	# Build CLI tool
	cargo_src_compile -p horcrux-cli

	# Build Web UI (requires trunk)
	if command -v trunk &> /dev/null; then
		cd horcrux-api/horcrux-ui || die
		trunk build --release || die "Failed to build Web UI"
		cd ../.. || die
	else
		ewarn "trunk not found, skipping Web UI build"
		ewarn "Install with: cargo install trunk"
	fi
}

src_install() {
	# Install binaries
	dobin target/release/horcrux-api
	dobin target/release/horcrux

	# Install Web UI
	if [[ -d horcrux-api/horcrux-ui/dist ]]; then
		insinto /opt/horcrux
		doins -r horcrux-api/horcrux-ui/dist
	fi

	# Install configuration
	insinto /etc/horcrux
	doins deploy/config/horcrux.toml

	# Install systemd services
	systemd_dounit deploy/systemd/horcrux-api.service
	systemd_dounit deploy/systemd/horcrux-metrics.service

	# Create directories
	keepdir /var/lib/horcrux/{vms,snapshots,backups,templates,cloudinit}
	keepdir /var/log/horcrux

	# Install documentation
	dodoc README.md
	dodoc docs/*.md

	# Install shell completions
	insinto /etc/bash_completion.d
	newins <(target/release/horcrux completions bash) horcrux

	if use zsh-completion; then
		insinto /usr/share/zsh/site-functions
		newins <(target/release/horcrux completions zsh) _horcrux
	fi
}

pkg_preinst() {
	# Create horcrux user and group
	enewgroup horcrux
	enewuser horcrux -1 -1 /var/lib/horcrux horcrux

	# Add to kvm group for VM management
	if getent group kvm &> /dev/null; then
		usermod -a -G kvm horcrux || die "Failed to add horcrux to kvm group"
	fi

	# Add to libvirt group if using libvirt
	if getent group libvirt &> /dev/null; then
		usermod -a -G libvirt horcrux || die "Failed to add horcrux to libvirt group"
	fi

	# Add to docker group if using docker
	if use docker && getent group docker &> /dev/null; then
		usermod -a -G docker horcrux || die "Failed to add horcrux to docker group"
	fi
}

pkg_postinst() {
	# Set ownership
	chown -R horcrux:horcrux /var/lib/horcrux || die
	chown -R horcrux:horcrux /var/log/horcrux || die

	# Generate random JWT secret if needed
	if grep -q "CHANGE_ME_TO_A_RANDOM_SECRET_KEY" /etc/horcrux/horcrux.toml; then
		local jwt_secret=$(openssl rand -base64 32)
		sed -i "s/CHANGE_ME_TO_A_RANDOM_SECRET_KEY/$jwt_secret/" /etc/horcrux/horcrux.toml
		einfo "Generated random JWT secret in /etc/horcrux/horcrux.toml"
	fi

	elog ""
	elog "Horcrux has been installed!"
	elog ""
	elog "Configuration: /etc/horcrux/horcrux.toml"
	elog "Data directory: /var/lib/horcrux"
	elog "Log directory: /var/log/horcrux"
	elog ""
	elog "To start Horcrux:"
	elog "  systemctl enable horcrux-api"
	elog "  systemctl start horcrux-api"
	elog ""
	elog "Web UI: http://localhost:8006"
	elog "API Docs: http://localhost:8006/api/docs"
	elog ""
	elog "Create admin user:"
	elog "  horcrux auth register"
	elog ""
	elog "For more information, see:"
	elog "  /usr/share/doc/${PF}/"
	elog ""
}
