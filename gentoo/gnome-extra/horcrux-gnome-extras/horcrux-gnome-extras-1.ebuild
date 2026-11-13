# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Horcrux full GNOME edition: the gnome-base/gnome extras (gnome-extra-apps) minus the apps Horcrux replaces, plus Maps and Music"
HOMEPAGE="https://horcrux.errant.solutions/"
S="${WORKDIR}"

LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64 arm64"

# Used together with gnome-base/gnome built with USE=-extras (variants/gnome/portage/package.use/horcrux-gnome): gnome-base/gnome[extras] would pull
# gnome-base/gnome-extra-apps, which has no way to leave one app out. This package is that list, kept in step with ::gentoo's gnome-extra-apps.
#
# DELIBERATELY NOT HERE (an app that duplicates something Horcrux already ships, or that does not work on Gentoo/OpenRC):
#   www-client/epiphany       the browser is Firefox
#   mail-client/evolution     owner machines run Thunderbird; the edition keeps evolution-data-server (Calendar/Contacts need it) but no second mail client
#   gnome-extra/gnome-logs    needs systemd (sys-apps/systemd, dbus[systemd]); journald does not exist on OpenRC
#   gnome-extra/gnome-software  PackageKit has no Portage backend: it can install nothing here (apps come from emerge)
#   gnome-extra/gnome-user-share  needs apache with WebDAV modules; off by default in ::gentoo (the `share` USE flag)
#   gnome-extra/gnome-tour, media-video/snapshot, gnome-extra/gnome-usage, gnome-extra/gnome-photos  not packaged in ::gentoo
#   media-video/showtime, media-sound/decibels (GURU)  duplicate Totem and Music, which gnome-core-apps/this package already install
RDEPEND="
	sys-apps/baobab
	media-video/cheese
	app-arch/file-roller
	gnome-extra/gnome-calculator
	gnome-extra/gnome-calendar
	gnome-extra/gnome-characters
	gnome-extra/gnome-clocks
	sys-apps/gnome-disk-utility
	media-gfx/gnome-font-viewer
	gnome-extra/gnome-system-monitor
	gnome-extra/gnome-weather
	gnome-extra/sushi
	net-misc/gnome-remote-desktop
	gnome-base/dconf-editor
	gnome-extra/gnome-tweaks
	app-crypt/seahorse
	net-misc/gnome-connections
	media-gfx/shotwell
	sci-geosciences/gnome-maps
	media-sound/gnome-music
	games-puzzle/five-or-more
	games-board/four-in-a-row
	games-board/gnome-chess
	games-puzzle/gnome-klotski
	games-board/gnome-mahjongg
	games-board/gnome-mines
	games-arcade/gnome-nibbles
	games-arcade/gnome-robots
	games-puzzle/gnome-sudoku
	games-puzzle/gnome-taquin
	games-puzzle/gnome-tetravex
	games-puzzle/hitori
	games-board/iagno
	games-puzzle/lightsoff
	games-puzzle/quadrapassel
	games-puzzle/swell-foop
	games-board/tali
"
