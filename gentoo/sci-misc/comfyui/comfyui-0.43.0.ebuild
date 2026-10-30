# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

PYTHON_COMPAT=( python3_13 )

inherit python-single-r1

DESCRIPTION="ComfyUI node-based diffusion UI/server (private venv with PyTorch cu130 wheels), Tailscale-only service"
HOMEPAGE="https://github.com/Comfy-Org/ComfyUI"
SRC_URI="https://github.com/Comfy-Org/ComfyUI/archive/refs/tags/v${PV}.tar.gz -> ComfyUI-${PV}.tar.gz"
S="${WORKDIR}/ComfyUI-${PV}"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS="~amd64"
# the venv is filled from PyPI + download.pytorch.org at build time (the binary package is what the laptop installs)
RESTRICT="network-sandbox mirror strip"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

RDEPEND="
	${PYTHON_DEPS}
	acct-user/comfyui
	acct-group/comfyui
	x11-drivers/nvidia-drivers
	net-vpn/tailscale
	net-misc/curl
	media-video/ffmpeg
"
BDEPEND="${PYTHON_DEPS} dev-python/pip"

QA_PREBUILT="usr/lib/comfyui/*"
QA_FLAGS_IGNORED="usr/lib/comfyui/.*"

src_compile() { :; }

src_install() {
	local app=/usr/lib/comfyui/app venv=/usr/lib/comfyui/venv
	insinto "${app}"
	doins -r .
	# user data lives in rpool/comfyui (COMFYUI_DATA); the package only owns code + venv
	${EPYTHON} -m venv --copies "${ED}${venv}" || die "venv"
	"${ED}${venv}/bin/python" -m pip install --no-cache-dir --upgrade pip || die "pip"
	"${ED}${venv}/bin/python" -m pip install --no-cache-dir \
		--extra-index-url https://download.pytorch.org/whl/cu130 \
		torch torchvision torchaudio -r requirements.txt || die "pip install (torch cu130 + ComfyUI requirements)"
	# nothing in the venv may point into the build image
	find "${ED}${venv}" -type f \( -name '*.pyc' -o -name 'RECORD' \) -delete
	grep -rIl "${ED}" "${ED}${venv}/bin" 2>/dev/null | while read -r f; do sed -i "s|${ED}||g" "$f"; done
	sed -i "s|${ED}||g" "${ED}${venv}/pyvenv.cfg" 2>/dev/null || true
	fperms 0755 "${app}/main.py"

	newinitd "${FILESDIR}/comfyui.initd" comfyui
	newconfd "${FILESDIR}/comfyui.confd" comfyui
	insinto /etc/comfyui
	doins "${FILESDIR}/extra_model_paths.yaml"
	dosbin "${FILESDIR}/comfyui-fetch-models"
	exeinto /usr/libexec
	doexe "${FILESDIR}/comfyui-launch"
}

pkg_postinst() {
	elog "ComfyUI listens on the Tailscale IPv4 address only (port 8188): rc-update add comfyui default"
	elog "Models: comfyui-fetch-models (manifest /usr/share/comfyui/models-manifest.tsv from sci-misc/comfyui-models-manifest or hogsmeade-tools)"
}
