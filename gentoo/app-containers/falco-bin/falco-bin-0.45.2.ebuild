# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

# Falco runtime threat detection, upstream's prebuilt glibc tarball.
# Horcrux (USE=runtime-detection) runs `falco` itself with the modern eBPF
# probe that is built into the binary, so this package ships NO init script or
# systemd unit: Horcrux supervises the process. Not installed on purpose:
#   - falcoctl (a self-updater/artifact installer; updates come through Portage)
#   - usr/src/falco-*+driver (kernel module sources, unused with modern eBPF)

DESCRIPTION="Cloud-native runtime security: syscall/container threat detection (prebuilt binary)"
HOMEPAGE="https://falco.org https://github.com/falcosecurity/falco"
SRC_URI="
	amd64? ( https://download.falco.org/packages/bin/x86_64/falco-${PV}-x86_64.tar.gz -> ${P}-amd64.tar.gz )
	arm64? ( https://download.falco.org/packages/bin/aarch64/falco-${PV}-aarch64.tar.gz -> ${P}-arm64.tar.gz )
"
S="${WORKDIR}"

LICENSE="Apache-2.0"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RESTRICT="mirror strip"

QA_PREBUILT="usr/bin/falco usr/share/falco/plugins/libcontainer.so"

src_unpack() {
	default
	# The tarball's top directory is falco-<PV>-<arch>.
	local d=( falco-${PV}-* )
	[[ -d ${d[0]} ]] || die "unexpected tarball layout"
	S="${WORKDIR}/${d[0]}"
}

src_install() {
	dobin usr/bin/falco

	exeinto /usr/share/falco/plugins
	doexe usr/share/falco/plugins/libcontainer.so

	# /etc/falco is config-protected by Portage (CONFIG_PROTECT covers /etc)
	insinto /etc/falco
	doins etc/falco/falco.yaml etc/falco/falco_rules.yaml etc/falco/falco_rules.local.yaml
	insinto /etc/falco/config.d
	doins etc/falco/config.d/falco.container_plugin.yaml
	keepdir /etc/falco/rules.d

	# Upstream's tarball bundles no licence text; Apache-2.0 comes from ::gentoo.
}

pkg_postinst() {
	elog "No init script is installed: Horcrux (USE=runtime-detection) starts and"
	elog "supervises falco with the built-in modern eBPF probe. Needs a kernel with"
	elog "BTF (CONFIG_DEBUG_INFO_BTF=y) and CONFIG_BPF_SYSCALL."
}
