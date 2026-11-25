# Copyright 2026 Horcrux Contributors
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit cargo git-r3

DESCRIPTION="Horcrux storage plugin: VM volumes in a directory on an SSH server (SSHFS)"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
EGIT_REPO_URI="https://github.com/CanuteTheGreat/horcrux.git"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""

# sshfs mounts the export, fusermount3 unmounts it, qemu-img makes the volumes
# and snapshots, ssh does the transport. No service: the manager runs the
# plugin per operation, so OpenRC and systemd nodes install the same files.
RDEPEND="
	app-emulation/qemu
	net-fs/sshfs
	net-misc/openssh
	sys-fs/fuse:3
"
BDEPEND="virtual/rust"

# the plugin is its own cargo workspace inside the Horcrux tree
S="${WORKDIR}/${P}/crates/horcrux-storage-plugin-sshfs"

src_unpack() {
	git-r3_src_unpack
	cargo_live_src_unpack
}

src_install() {
	exeinto /usr/libexec/horcrux/storage-plugins
	newexe "$(cargo_target_dir)"/horcrux-storage-plugin-sshfs sshfs
	dodoc "${WORKDIR}/${P}"/docs/storage-plugins.md "${WORKDIR}/${P}"/docs/storage-plugin-api-changelog.md
}

pkg_postinst() {
	elog "SSHFS is now a storage type in Storage > Create Pool (no restart needed)."
	elog "Every node that uses the pool needs SSH access to the server (key in"
	elog "the key file named in the pool, or the node's default key)."
}
