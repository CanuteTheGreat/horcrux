# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): Ollama on the RTX 5090 for crystalcave's local-first LLM gates,
# opportunistic. The guard starts `ollama serve` (loopback only) only while app-misc/hogsmeade-ci-idle-gate's gate says he is away (15 min no input, no game, ComfyUI idle,
# memory headroom, no foreign GPU memory) and kills it (VRAM freed, nothing resident) within ~1 min of him coming back. Init-neutral (OpenRC + systemd).
EAPI=8

DESCRIPTION="HOGSMEADE: idle-gated Ollama (GPU freed when the owner is back)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-misc/hogsmeade-ci-idle-gate
	sci-ml/ollama-bin
	sys-apps/util-linux
	net-misc/curl
"

src_install() {
	exeinto /usr/libexec
	doexe "${FILESDIR}"/hogsmeade-llm-guard "${FILESDIR}"/hogsmeade-llm-pull
	newinitd "${FILESDIR}"/hogsmeade-llm-guard.initd hogsmeade-llm-guard
	newconfd "${FILESDIR}"/hogsmeade-llm-guard.confd hogsmeade-llm-guard
	insinto /usr/lib/systemd/system
	doins "${FILESDIR}"/hogsmeade-llm-guard.service
}

pkg_postinst() {
	elog "Models: hogsmeade-llm-pull (as root, idle I/O); enable: rc-update add hogsmeade-llm-guard default && rc-service hogsmeade-llm-guard start"
	elog "(systemd: systemctl enable --now hogsmeade-llm-guard). Ollama listens on 127.0.0.1:11434 only; crystalcave reaches it with ssh -L as hogsmeade-agent."
}
