# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the USE flags for app-misc/hogsmeade-microsoft. A separate package because Portage must see these files BEFORE it can resolve
# the Microsoft 365 software (a package cannot carry the USE it is resolved with; same pattern as app-misc/hogsmeade-apps-config).
EAPI=8

DESCRIPTION="HOGSMEADE: package.use for the Microsoft 365 software (app-misc/hogsmeade-microsoft)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

src_install() {
	insinto /etc/portage/package.use
	newins - hogsmeade-microsoft <<-'EOF2'
		# HOGSMEADE (app-misc/hogsmeade-microsoft-config): GNOME Online Accounts 'Microsoft 365', the Files OneDrive mount (gvfs -> net-libs/msgraph), onedrive desktop notifications
		net-libs/gnome-online-accounts ms365
		gnome-base/gvfs onedrive gnome-online-accounts
		net-misc/onedrive libnotify
	EOF2
}
