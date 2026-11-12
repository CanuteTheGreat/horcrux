# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY: the Steam client. Needs ::steam-overlay synced first (app-misc/hogsmeade-steam-repo registers it).
EAPI=8

DESCRIPTION="HOGSMEADE: Steam client (from ::steam-overlay) with its 32-bit dependencies"
S="${WORKDIR}"
LICENSE="GPL-2"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-misc/hogsmeade-steam-repo
	games-util/steam-launcher
"
