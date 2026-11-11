# Copyright 2026 Errant Solutions
# PRIVATE, machine-local package (never in a public ISO): the agent's ssh public key + the machine's network defaults (ComfyUI on the home LAN behind a host firewall).
# Backup credentials are NEVER baked in (provisioned after first boot from crystalcave). The payload is copied into files/ by the private image build only.
EAPI=8

DESCRIPTION="HOGSMEADE private payload: agent ssh public key (NO restic credentials), ComfyUI LAN bind + host firewall"
S="${WORKDIR}"
LICENSE="all-rights-reserved"
RESTRICT="bindist mirror"
SLOT="0"
KEYWORDS="~amd64"
RDEPEND="net-firewall/nftables"

src_install() {
	insinto /etc/hogsmeade
	insopts -m0600
	local f
	# hostname = the owner's name for this machine (PRIV_DIR/hostname): hogsmeade-install shows it prefilled and the owner confirms it
	for f in hostname; do
		[[ -s ${FILESDIR}/${f} ]] && newins "${FILESDIR}/${f}" "${f}"
	done
	[[ -s ${FILESDIR}/hostname ]] || die "the private payload has no hostname file (the owner's name for this machine)"
	# the agent's key: root (key-only sshd); hogsmeade-firstboot copies it to the desktop user once that user exists
	if [[ -s ${FILESDIR}/agent_authorized_keys ]]; then
		newins "${FILESDIR}/agent_authorized_keys" agent_authorized_keys
		insinto /root/.ssh
		newins "${FILESDIR}/agent_authorized_keys" authorized_keys
	fi

	# ComfyUI without Tailscale (decision 2026-11-10): bound to all addresses, port 8188 reachable ONLY from this machine, the home LAN
	# (10.66.69.0/24) and the home WireGuard network (10.69.69.0/24). ComfyUI has no login: the service refuses to start without the firewall
	# (rc_need). Own nftables table, so Docker's rules and any other ruleset are untouched.
	insinto /etc/hogsmeade
	insopts -m0644
	newins - firewall.nft <<-'EOF'
		# HOGSMEADE host firewall (sys-apps/hogsmeade-private): loaded by /etc/init.d/hogsmeade-firewall
		table inet hogsmeade
		delete table inet hogsmeade
		table inet hogsmeade {
			set comfyui_allowed {
				type ipv4_addr
				flags interval
				elements = { 10.66.69.0/24, 10.69.69.0/24 }
			}
			chain input {
				type filter hook input priority filter - 5; policy accept;
				iifname "lo" accept
				tcp dport 8188 ip saddr @comfyui_allowed accept
				tcp dport 8188 counter reject with tcp reset
			}
		}
	EOF
	newinitd - hogsmeade-firewall <<-'EOF'
		#!/sbin/openrc-run
		description="HOGSMEADE host firewall (ComfyUI 8188: loopback, home LAN and home WireGuard only)"
		depend() { need localmount; before net comfyui; }
		start() {
			ebegin "Loading the HOGSMEADE firewall table"
			nft -f /etc/hogsmeade/firewall.nft && nft list chain inet hogsmeade input >/dev/null
			eend $?
		}
		stop() {
			ebegin "Removing the HOGSMEADE firewall table"
			nft delete table inet hogsmeade 2>/dev/null
			eend 0
		}
	EOF
	insinto /etc/conf.d
	newins - comfyui.local <<-'EOF'
		# HOGSMEADE (sys-apps/hogsmeade-private), read by /etc/conf.d/comfyui: reachable from the home LAN / WireGuard, never without the firewall
		COMFYUI_LISTEN="0.0.0.0"
		rc_need="hogsmeade-firewall"
	EOF
}

pkg_postinst() {
	chmod 0700 "${ROOT}"/root/.ssh 2>/dev/null
}
