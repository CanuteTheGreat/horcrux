# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux role: system + application containers (Incus, Podman, Buildah, Skopeo, LXC) next to the server core's Docker"
HOMEPAGE="https://github.com/CanuteTheGreat/horcrux"
S="${WORKDIR}"

# A ROLE PACKAGE (docs/ROLES.md): a node that runs containers installs this one atom instead of picking runtimes by hand.
# Lab finding 2026-11-05: Incus / Podman / Buildah were installed on no laptop edition, so the container tests (hx-containers) could only run Docker and the
# Horcrux manager's incus/podman/lxc backends (USE incus podman lxc of app-emulation/horcrux) had nothing to talk to.
# Not in the server core on purpose: the core stays small; a hypervisor-only node does not need five more runtimes.
# The keywords these runtimes need (testing on ::gentoo) come with sys-apps/horcrux-repo-config (/etc/portage/package.accept_keywords/horcrux-roles).
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

RDEPEND="
	app-containers/buildah
	app-containers/incus
	app-containers/lxc
	app-containers/podman
	app-containers/skopeo
	sys-apps/shadow
"

src_install() {
	insinto /usr/share/horcrux
	newins - role-containers <<-EOF
		horcrux-role-containers ${PVR}
	EOF
}

pkg_postinst() {
	elog "Container role installed. Enable what you use (none of it is started by default):"
	elog "  rc-update add incus default && rc-service incus start      # then: incus admin init --minimal"
	elog "  Podman is daemonless; rootless needs subuid/subgid for the user:  usermod --add-subuids 100000-165535 --add-subgids 100000-165535 <user>"
	elog "  Buildah / Skopeo: nothing to start."
}
