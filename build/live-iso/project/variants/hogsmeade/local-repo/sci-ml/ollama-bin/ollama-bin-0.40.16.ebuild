# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (hogsmeade variant local repo): upstream's prebuilt Ollama with its bundled CUDA 12/13 runners. ::gentoo's sci-ml/ollama is ~amd64 and would compile the
# CUDA kernels on the owner's desktop (no CUDA toolkit installed there), so the vendor tarball is unpacked into ${D} like the other -bin packages. It is only ever
# started by app-misc/hogsmeade-local-llm (idle-gated), never by an init script of its own.
EAPI=8

inherit unpacker

DESCRIPTION="Ollama (prebuilt, bundled CUDA runners) for HOGSMEADE"
HOMEPAGE="https://ollama.com/"
SRC_URI="https://github.com/ollama/ollama/releases/download/v${PV}/ollama-linux-amd64.tar.zst -> ${P}-amd64.tar.zst"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="-* amd64"
RESTRICT="strip mirror"
QA_PREBUILT="*"
BDEPEND="app-arch/zstd"
RDEPEND="acct-user/ollama x11-drivers/nvidia-drivers"
# a second Ollama would fight over /usr/bin/ollama
RDEPEND+=" !sci-ml/ollama"

src_install() {
	insinto /opt/ollama-bin
	doins -r lib
	exeinto /opt/ollama-bin/bin
	doexe bin/ollama
	dosym ../../opt/ollama-bin/bin/ollama /usr/bin/ollama
}
