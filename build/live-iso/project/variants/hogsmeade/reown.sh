#!/bin/bash
# reown.sh: give every file that no Portage package owns (installer overlay, live init scripts, desktop markers, setup-rootfs output)
# to a generated machine-local package sys-apps/hogsmeade-image-files, so that `qfile -o` is clean and the files are tracked.
# Runtime/generated paths are excluded (see EXCL). Prints the orphan counts. Runs at the very end of the image build.
set -euo pipefail
REPO=/var/db/repos/local/sys-apps/hogsmeade-image-files
EXCL='^/(proc|sys|dev|run|tmp|var/tmp|var/cache|var/log|var/db|var/lib/portage|var/lib/dbus|var/lib/systemd|var/lib/NetworkManager|var/lib/misc|var/spool|var/mail|lost\+found|mnt|media|srv|home|root|boot|usr/src|etc/portage|etc/runlevels|var/db/repos|lib/modules|usr/share/mime|usr/share/icons/[^/]+/icon-theme.cache|usr/lib/(python[0-9.]+|locale)/|usr/lib/gio/modules/giomodule.cache|usr/lib/gdk-pixbuf|usr/lib64/gdk-pixbuf|usr/lib/udev/hwdb.bin|usr/share/applications/mimeinfo.cache|usr/share/glib-2.0/schemas/gschemas.compiled|usr/lib/gtk-4.0|usr/lib/gtk-3.0|usr/lib/graphene|etc/ld.so.cache|etc/(passwd|group|shadow|gshadow|subuid|subgid|hostname|machine-id|resolv.conf|mtab|adjtime|localtime|timezone|locale.gen|env.d/.*|ssl/certs/.*|ca-certificates.*|fonts/conf.d/.*|udev/hwdb.bin|profile.env|csh.env|ld.so.conf.d/.*|shells|conf.d/hostname|conf.d/display-manager|conf.d/net|init.d/net.*|\.pwd.lock|\.updated|openrc/.*)|usr/share/(fonts/.*/fonts.(dir|scale)|\.fonts-cache.*|info/dir)|var/lib/(alternatives|ca-certificates|iwd|ntp|polkit-1|gdm|upower|cups|colord|bluetooth|tailscale|docker|libvirt|chrony|clamav)/?.*)'
mkdir -p /tmp/reown && cd /tmp/reown
find /usr /opt /etc /lib/firmware -xdev \( -type f -o -type l \) -print0 2>/dev/null | xargs -0 qfile -o 2>/dev/null | grep '^/' | sed 's/ *$//' | grep -v -E "$EXCL" | grep -v '/__pycache__/' | sort -u > orphans.txt || true
echo "reown: $(wc -l < orphans.txt) unowned paths outside the exclusion list"
head -50 orphans.txt
[ -s orphans.txt ] || { echo "reown: nothing to do"; exit 0; }
mkdir -p "$REPO/files"
# directories listed by qfile -o are only kept when they are leaves; tar the files and let tar create the parents
tar --no-recursion --null -T <(tr '\n' '\0' < orphans.txt) -cpf "$REPO/files/payload.tar" 2>/dev/null
cp orphans.txt "$REPO/files/orphans.txt"
cat > "$REPO/hogsmeade-image-files-1.ebuild" <<'EOF'
# Copyright 2026 Errant Solutions
# Distributed under the terms of the GNU General Public License v2
EAPI=8
DESCRIPTION="Files of the Horcrux HOGSMEADE image that have no upstream package (installer overlay, live/init scripts, desktop markers)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64"
RESTRICT="strip"
src_install() {
	tar -x -C "${ED}" -pf "${FILESDIR}/payload.tar" || die
}
EOF
cat > "$REPO/metadata.xml" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE pkgmetadata SYSTEM "https://www.gentoo.org/dtd/metadata.dtd">
<pkgmetadata><maintainer type="person"><email>hermes-agent@errant.solutions</email></maintainer></pkgmetadata>
EOF
ebuild "$REPO/hogsmeade-image-files-1.ebuild" manifest
CONFIG_PROTECT="-*" CONFIG_PROTECT_MASK="/" emerge --oneshot --select=y sys-apps/hogsmeade-image-files
echo "reown: remaining orphans after packaging: $(find /usr /opt /etc /lib/firmware -xdev \( -type f -o -type l \) -print0 2>/dev/null | xargs -0 qfile -o 2>/dev/null | grep '^/' | grep -v -E "$EXCL" | grep -v '/__pycache__/' | wc -l)"
