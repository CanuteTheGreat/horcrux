# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the owner's SSH CA client set-up on his main work laptop (never in a public image, the Horcrux overlay or a lab/test machine).
# Pulls the personal-CA client (net-misc/ctg-ssh-ca: cron renew), its private fleet data (net-misc/ctg-ssh-ca-fleet: host-CA @cert-authority, user CA, KRL,
# signer address, aliases) and app-admin/ctg-ops[private-overlay] (::ctg-private sync), and owns the RedWeek side: its own, SEPARATE user CA issues a
# per-device certificate for the same device key (owner rule: the personal CA never signs RedWeek access).
# NO KEY OR CERTIFICATE IS PACKAGED: the device key is generated ON the laptop; after an install run `ctg-ssh-ca enroll` (personal CA, approved on
# crystalcave) and have the agent issue the RedWeek device cert (redweek-devops packaging/aws/ssh-user-ca/issue-user.sh) into ~/.ssh/id_ed25519-rw-cert.pub.
EAPI=8

DESCRIPTION="HOGSMEADE: SSH CA client set-up (personal CA + RedWeek device cert renewal, host-CA trust for RedWeek)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-admin/ctg-ops[private-overlay]
	net-misc/ctg-ssh-ca
	net-misc/ctg-ssh-ca-fleet
	net-misc/curl
	net-misc/openssh
	dev-lang/python
	|| ( dev-util/xxd app-editors/vim-core )
	virtual/cron
"

src_install() {
	dobin "${FILESDIR}/rw-usercert-renew"

	# RedWeek host CA (host certificates of rw-* hosts) next to the personal one in /etc/ctg-ssh-ca/ca_known_hosts (GlobalKnownHostsFile
	# of ctg-ssh-ca-fleet lists /etc/ssh/ssh_known_hosts first). CA fingerprint SHA256:sPVYw5Njq3QLqXmlKa+7okQS0mUfIOI3oE5iF/SLawM.
	insinto /etc/ssh
	newins - ssh_known_hosts <<-'EOF2'
		@cert-authority *.redweek.internal,*.redweektest.com,*.redweek.com,rw-*,10.*,172.31.*,216.211.130.*,3.*,18.*,34.*,35.*,44.*,52.*,54.* ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG8pbjs2TZgXALy6OB/q/0i7zt7PWeSDFl27wmvuuNBh RedWeek SSH host CA (2026-11)
	EOF2

	# the RedWeek device certificate is offered ONLY to RedWeek names; everything else uses the personal one (id_ed25519-cert.pub, automatic)
	insinto /etc/ssh/ssh_config.d
	newins - 50-hogsmeade-redweek.conf <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-ssh-ca): RedWeek user CA device certificate for the SAME device key
		# (the alias the owner typed, e.g. rw-man-02 whose HostName is an AWS public IP, and the resolved colo/AWS addresses)
		Match originalhost "rw-*,*.redweek.internal,*.redweek.com,*.redweektest.com"
		    CertificateFile ~/.ssh/id_ed25519-rw-cert.pub
		Match host "10.1.*,172.31.*,216.211.130.*,44.238.163.95,52.43.196.131,32.191.129.219,52.25.28.82"
		    CertificateFile ~/.ssh/id_ed25519-rw-cert.pub
	EOF2

	# renewal needs no AWS credentials (proof of possession); renews below 30 days left, a missing cert is a quiet no-op
	insinto /etc/cron.d
	newins - rw-usercert-renew <<-'EOF2'
		# HOGSMEADE: renew the RedWeek device certificate (renews when < 30 days are left)
		41 5 * * * canutethegreat [ -s /home/canutethegreat/.ssh/id_ed25519-rw-cert.pub ] && /usr/bin/rw-usercert-renew >/dev/null 2>&1
	EOF2
}
