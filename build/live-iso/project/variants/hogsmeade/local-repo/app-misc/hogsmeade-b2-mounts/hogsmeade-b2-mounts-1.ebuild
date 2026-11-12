# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): his three Backblaze B2 buckets as geesefs
# mounts under /mnt (NOT in $HOME). Same geesefs options as crystalcave/the Mac (stat-cache-ttl 10m, memory-limit 1000, read-ahead-large 204800),
# at low CPU/IO priority, started by OpenRC after the network with an endless retry (never blocks boot or login, survives Wi-Fi/wired changes).
# Credentials are NOT packaged: /root/.aws/credentials, 0600, one profile per bucket, a restricted key per bucket (hogsmeade-<bucket>-rw).
EAPI=8

DESCRIPTION="HOGSMEADE: geesefs mounts of the B2 buckets ctg-storage, ctg-media and ctg-hacktheplanet at /mnt/<bucket>"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"

RDEPEND="
	app-misc/hogsmeade-apps-config
	net-fs/geesefs
	sys-fs/fuse:0
	sys-apps/util-linux
"

BUCKETS=(ctg-storage ctg-media ctg-hacktheplanet)

src_install() {
	newinitd "${FILESDIR}"/hogsmeade-b2-mount.initd hogsmeade-b2-mount
	newconfd "${FILESDIR}"/hogsmeade-b2-mount.confd hogsmeade-b2-mount
	exeinto /usr/libexec
	doexe "${FILESDIR}"/hogsmeade-b2-mount
	local b
	for b in "${BUCKETS[@]}"; do
		dosym hogsmeade-b2-mount "/etc/init.d/hogsmeade-b2-mount.${b}"
	done
	# no keepdir under /mnt: the dirs are live geesefs mountpoints and Portage cannot write a .keep inside them
}

# GNOME Files (gvfs) lists a mount outside /media and $HOME only if /etc/fstab has an entry with x-gvfs-show for its mount point. The entries are
# display hints only (noauto: nothing mounts from fstab, the OpenRC services above do), kept in one marked block that this package adds and removes.
_fstab_block() {
	local f=${EROOT}/etc/fstab t b
	[[ -f ${f} ]] || return 0
	t=$(mktemp) || return 0
	sed '/^# BEGIN hogsmeade-b2-mounts/,/^# END hogsmeade-b2-mounts/d' "${f}" > "${t}"
	if [[ $1 == add ]]; then
		{
			echo "# BEGIN hogsmeade-b2-mounts (app-misc/hogsmeade-b2-mounts; GNOME Files display hints only, mounted by the hogsmeade-b2-mount.<bucket> services)"
			for b in "${BUCKETS[@]}"; do
				echo "${b} /mnt/${b} fuse.geesefs noauto,nofail,x-gvfs-show,x-gvfs-name=${b} 0 0"
			done
			echo "# END hogsmeade-b2-mounts"
		} >> "${t}"
	fi
	cat "${t}" > "${f}"
	rm -f "${t}"
}

pkg_postinst() {
	local b
	_fstab_block add
	for b in "${BUCKETS[@]}"; do
		[[ -e ${EROOT}/mnt/${b} ]] || mkdir -p "${EROOT}/mnt/${b}" 2>/dev/null
	done
	elog "Credentials are not packaged: /root/.aws/credentials (0600) needs a [ctg-storage], [ctg-media] and [ctg-hacktheplanet] profile (keys hogsmeade-<bucket>-rw)."
	elog "Enable: for b in ${BUCKETS[*]}; do rc-update add hogsmeade-b2-mount.\${b} default; rc-service hogsmeade-b2-mount.\${b} start; done"
}

pkg_postrm() {
	[[ -z ${REPLACED_BY_VERSION} ]] && _fstab_block remove
}
