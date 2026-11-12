# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's personal app, never in a public image, the Horcrux overlay or a lab/test machine): the Nextcloud desktop client with the
# Files (nautilus) integration. This package is the layer's "set" for it (RDEPEND = the app, so @world keeps it) and owns its USE configuration.
EAPI=8

DESCRIPTION="HOGSMEADE: Nextcloud desktop client (GNOME Files integration)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

# the USE requirement mirrors the package.use file below (the image build resolves it from the variant's own portage/ files first)
RDEPEND="net-misc/nextcloud-client[nautilus]"

src_install() {
	insinto /etc/portage/package.use
	newins - nextcloud-client <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-nextcloud)
		net-misc/nextcloud-client nautilus
	EOF2
}
