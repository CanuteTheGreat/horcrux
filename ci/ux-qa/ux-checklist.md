# UX review checklist (fresh-eyes review of screenshots / CLI text)

Reviewer persona: a demanding Proxmox/pfSense admin on day one with no knowledge of how this product was built. Judge what
is on screen, not what the code intends. The deterministic crawl (ux_crawl.py) already caught JS errors, overlays on canvases,
5xx, sideways scroll, private URLs; do NOT repeat those. Look for what only eyes find.

For every screenshot answer each question; report only YES findings (page, viewport, scheme, one-line what, one-line fix).

1. **Hidden content**: does anything (toolbar, banner, toast, sticky header, floating button, modal) cover content a user needs, including the bottom rows of a table or the guest screen of a console? Is text cut off or clipped at the phone width?
2. **Dead summaries**: is there a number, card, badge, row, chart legend or breadcrumb that looks clickable (hover style, arrow, count) but a click would obviously be expected to go somewhere (a list filtered to that number)? Does a count disagree with the list it summarises?
3. **Jargon / internal names**: build or repo vocabulary a user never chose (edition codenames like "light", "live", "installer", "openrc", branch/job names, machine names, raw enum values `VmState::Running`, `null`, `undefined`, UUIDs where a name belongs, Rust/serde error text).
4. **Private/internal URLs or hosts**: any git/forge/CI/internal domain, RFC1918 address the user did not enter, a personal machine name.
5. **Inconsistent naming**: the same thing called two names (VM / guest / instance; storage / datastore / pool; node / host), different capitalisation of the same label, one page "Backups", its menu entry "Snapshots & Backup".
6. **Missing actions a Proxmox user expects**: a VM list without start/stop/console/migrate/clone/snapshot/backup on the row or in a menu; storage page with no add/remove; a network page without bridge/VLAN create; cluster page without join; tasks page without log/cancel; no bulk select; no search/filter on a long list; no way to export/download.
7. **Empty states**: a blank table or "No X found." without saying why and offering the next step (a "Create X" / "Import X" / "Learn how" button). Empty state that looks like a loading failure.
8. **Error text**: an error banner or toast that does not say what happened AND what to do; a bare "Failed", "500", "Error: ..." with no cause; a raw backend message; an error with no retry. Warnings that never go away.
9. **Visual consistency & polish**: wrong contrast in dark mode (light panels on dark, dark text on dark), unreadable disabled controls, icons without labels, misaligned cards, inconsistent button styles, double scroll bars, tiny tap targets (< 40 px) at phone width, horizontal scroll, overlapping text.
10. **First-run clarity**: would a new user know the first thing to do from the dashboard? Is the product name/version visible somewhere (about / footer)? Is there a way to log out / change password / get help?
11. **Phone layout**: is the navigation reachable (menu button), are tables usable (scroll inside a container or cards), are primary actions reachable without zooming?
12. **Dark vs light parity**: anything that exists in one scheme only, illegible in either.

CLI / OS-level text (os-release, /etc/motd, issue, login banner, installer prompts, `--help`):
13. No "live", "installer", hostname-as-edition, internal build names, private URLs on an INSTALLED system (os-release/motd/issue).
14. Prompts say what will happen and what the default is; destructive steps name the device and require an explicit confirmation; errors say what to do next.
15. Version and edition are stated consistently across os-release, motd, UI footer/about, and package metadata.

Output format (JSON lines, one per finding):
`{"id":"ux-<short-slug>","page":"/vms","viewport":"phone","scheme":"dark","check":3,"what":"...","fix":"...","kind":"bug|judgement","files":"likely source file(s)"}`
`kind: judgement` = a product decision (naming, missing feature, layout preference); `bug` = unambiguous defect with a known fix.
