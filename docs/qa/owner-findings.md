# Owner-found issues and the checks that cover their class

Rule (owner, 2026-11-04): every project gets automatic QA that finds things so the owner does not have to. When the owner
finds a bug by hand, add a check for that CLASS of bug, then add a row here. `ci/ux-qa/check_owner_findings.py` (run in CI)
fails when a row names no check or a check file that does not exist. `pending:<branch>` = check in flight on that branch
(warns until it lands; replace with the path).

| # | date | finding (as the owner saw it) | class | check(s) |
|---|------|-------------------------------|-------|----------|
| 1 | 2026-11-12 | VNC console button overlay covered the guest screen | an element overlays a console canvas/iframe/video | `horcrux-api/horcrux-ui/e2e/console_overlay_e2e.py`, `ci/ux-qa/ux_crawl.py` (canvas-overlay on every page) |
| 2 | 2026-11-12 | Dashboard cards/stats were not clickable | summary/stat card or list item that is not a working link | `ci/ux-qa/ux_crawl.py` (unlinked-summary, fails on every page), pending:dash-links (e2e/dashboard_links_e2e.py: each card navigates) |
| 3 | 2026-11-12 | Installed system's /etc/os-release said live/installer, had the hostname as VARIANT, said "GNOME Light", HOME_URL = private Forgejo | os-release sanity on installed systems | `build/live-iso/common/tests/test_os_release.py`, `ci/ux-qa/check_os_release.py` (any rootfs/file; run by the UX lane on the real install) |
| 4 | 2026-11-12 | Private Forgejo URL in a public artifact | private URL/host in any public artifact (images, os-release, UI, docs, package metadata) | `gentoo/ci/check-no-forgejo.py`, `ci/ux-qa/check_public_text.py` (dirs, tarballs, wasm/binaries; machine names fail inside the UI bundle) |
| 5 | 2026-11-12 | Blank web UI after a hand-deployed build (wasm `Table.grow`) | UI does not load / uncaught JS error in real Firefox | `horcrux-api/horcrux-ui/e2e/ui_load_check.py`, `ci/ux-qa/ux_crawl.py` (js-error, blank, on every page, desktop+phone, light+dark) |
| 6 | 2026-11-12 | libvirt VMs invisible: no importer | unmanaged/foreign VMs not detected and offered | `ci/ux-qa/check_foreign_vms.py` (fake virsh, UI offer on /vms) |
| 7 | 2026-11-12 | Docker's FORWARD DROP silently killed VM NAT | VM NAT egress with Docker present | `build/live-iso/common/headless_gates.py` (gate docker-does-not-break-vm-nat-forwarding), `build/live-iso/common/tests/test_headless_gates.py` |
| 8 | 2026-11-12 | Teams/Electron scaling regression (tiny X11 apps under fractional scaling) | desktop defaults regress app rendering | `build/live-iso/common/tests/test_fractional_scaling.py` |
| 9 | 2026-11-12 | Slack tray icon missing (GNOME AppIndicator vs Electron SNI) | desktop apps' tray icons | `build/live-iso/common/tests/test_sni_chromium.py`, `build/live-iso/common/tests/sni_headless_gate.sh` |
| 10 | 2026-11-12 | (found by the UX lane, not the owner) API answers 500/503 instead of an empty state when docker/journald/nft is absent: Containers, Firewall log, Services, Logs pages | API 5xx on a normal page load | `ci/ux-qa/ux_crawl.py` (http-5xx, baseline now empty: a missing engine answers 424 NOT_AVAILABLE and the UI shows an empty state; `horcrux-api/horcrux-ui/src/components/unavailable.rs`) |
| 11 | 2026-11-12 | Nav bar broken at 1280px: dropdown arrows on their own line, 'System Alerts' wrapped, 'Administration' clipped, tagline on 3 lines | nav item wrapped/clipped at 1280 or 1440px | `ci/ux-qa/ux_crawl.py` (nav-broken, desktop + wide widths) |
| 12 | 2026-11-12 | 'Active Alerts 0' painted red on the dashboard | a stat card showing 0/none in red (red is only for > 0) | `ci/ux-qa/ux_crawl.py` (zero-red), `ci/ux-qa/test_ux_qa.py` |
| 13 | 2026-11-12 | (UX lane) HA pages unstyled and clipped on phones; /auth/roles scrolls sideways; every page has the same title; empty states without a next action; stat cards on other pages not links | unstyled page, horizontal overflow, duplicate titles, empty state without action, unlinked stat card | `ci/ux-qa/ux_crawl.py` (overflow fails at all widths, same-title, empty-no-action, unlinked-summary now fail), `horcrux-api/horcrux-ui/src/titles.rs` (every route has a unique title) |
| 14 | 2026-11-13 | Dashboard flagged the sources of completed libvirt imports as 'unmanaged' (and offered to re-import them) next to the 'old sources can be cleaned up' line | an imported source listed as unmanaged / re-importable | `horcrux-api/src/api/libvirt_import.rs` (`a_recorded_import_source_is_not_unmanaged`), `horcrux-api/horcrux-ui/e2e/unmanaged_libvirt_banner_e2e.py` (mocked list: only the genuine domain in the banner) |
| 14 | 2026-11-12 | (UX crawl) Dashboard / System Alerts sat 5 px lower than the dropdown triggers; the menu ran under the username / Log out at 1280 | nav items misaligned or overlapping each other in the bar | `ci/ux-qa/ux_crawl.py` (nav-broken: one row bucket, and the new pairwise overlap check of brand, menu items and user menu) |
| 15 | 2026-11-12 | (UX crawl) NAS Create Share / Create Pool / Add User linked to routes that did not exist; creating a share failed (no owner_user column) and the share list never parsed | an action link to a route the app does not have; a form whose API call fails | `ci/ux-qa/ux_crawl.py` (same-title: unknown routes fall back to one title), `horcrux-api/horcrux-ui/e2e/ui_e2e.py` (nas_share_checks: create, edit, delete through the UI) |
| 16 | 2026-11-12 | (UX crawl) /nas/pools, /nas/iscsi and /system/config answered 500/503 on a node without zpool, tgtadm or iproute2 | a missing node tool answered as a server error instead of a 424 empty state | `ci/ux-qa/ux_crawl.py` (http-5xx), `horcrux-api/src/api/unavailable.rs` (a_missing_tool_is_a_424_not_a_500) |

## How the loop runs

- **Per push (CI):** `ci/ux-qa/test_ux_qa.py` (checker self-tests), `check_public_text.py` on docs, `check_owner_findings.py`,
  `ux_crawl.py` against the built UI (every nav page, desktop + 1440 + phone, light + dark), `check_foreign_vms.py`.
- **Per release (QA cron `horcrux-ux-review`, gated on a new version):** deploys the release build to a throwaway
  container on rw-man-01, runs `ux_crawl.py` with screenshots, `check_public_text.py` on the release tarball, then a Sonnet
  vision review of the screenshots against `ci/ux-qa/ux-checklist.md`. Fixable findings are fixed with a test; judgement
  calls go in the daily summary.
- **When the owner reports something:** the product-qa lane adds the check for the class, adds a row here, and links the commit.
