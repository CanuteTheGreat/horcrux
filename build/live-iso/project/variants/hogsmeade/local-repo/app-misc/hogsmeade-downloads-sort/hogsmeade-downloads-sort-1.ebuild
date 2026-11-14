# Copyright 2026 Errant Solutions
# HOGSMEADE-ONLY (owner's machine, never in a public image, the Horcrux overlay or a lab machine): keeps the owner's ~/Downloads
# (a link into ~/OneDrive-Personal/Downloads) sorted into plain type folders (Documents, Images, Videos, Archives, Installers/<OS>, Code,
# Software, Folders, Other ...), owner request 2026-11-13. MOVES ONLY: never deletes or overwrites (rename with RENAME_NOREPLACE,
# " (2)" on a clash), every move logged to ~/.local/state/downloads-sort/moves-YYYYMMDD.tsv with an undo-YYYYMMDD.sh. Files and
# folders touched in the last 14 days stay at the top level. It waits for the onedrive client: destination folders must be online
# before anything moves into them, otherwise onedrive re-uploads the data instead of doing a server-side move.
# Scheduling is init-neutral: an /etc/cron.d entry (works the same on OpenRC and systemd); hourly check, one real run per 20 h,
# so a laptop that sleeps at night still gets its nightly run.
EAPI=8

PYTHON_COMPAT=( python3_{12..14} )
inherit python-single-r1

DESCRIPTION="HOGSMEADE: nightly sorter for ~/Downloads (moves only, logged, undoable, OneDrive-aware)"
S="${WORKDIR}"
LICENSE="MIT"
SLOT="0"
KEYWORDS="amd64"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

RDEPEND="
	${PYTHON_DEPS}
	sys-apps/util-linux
	virtual/cron
"

src_install() {
	python_newscript "${FILESDIR}"/hogsmeade-downloads-sort hogsmeade-downloads-sort
	insinto /etc/cron.d
	newins "${FILESDIR}"/hogsmeade-downloads-sort.cron hogsmeade-downloads-sort
}

pkg_postinst() {
	elog "Runs from /etc/cron.d/hogsmeade-downloads-sort as canutethegreat (hourly check, at most one run per 20 h)."
	elog "Moves only; log + undo script: ~/.local/state/downloads-sort/ (moves-YYYYMMDD.tsv, undo-YYYYMMDD.sh, progress.log)."
	elog "By hand: hogsmeade-downloads-sort plan | apply | verify | report  (report lists duplicates/Windows installers, deletes nothing)."
}
