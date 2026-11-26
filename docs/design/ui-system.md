# Horcrux UI system

One design system for the Horcrux web UI (`horcrux-api/horcrux-ui`). Every page is built from the
tokens and components below; nothing in a page sets its own colours, font sizes or spacing.

The bar is the leading self-hosted virtualization consoles: an operator managing dozens of nodes and
hundreds of guests must see state at a glance, act without hunting, and never wonder whether a page
is live. The look is our own (calm graphite neutrals, a verdigris accent, the Horcrux crest in gold);
no other product's assets, icons or branding are used.

Sources:
- `ui-kit/` at the repo root: the **shared, framework-neutral kit** (`tokens.css`, `components.css` with
  `ui-`-prefixed classes, optional `ui.js`, `gallery.html`, `check.py`, markup contract in `ui-kit/README.md`).
  Patronus (askama templates, no build step) consumes the same files; Horcrux links them from `index.html`.
- `horcrux-api/horcrux-ui/style/ui-system.css`: the Horcrux layer on top of the kit (bundled IBM Plex type,
  app shell, and the mapping of the older page classes onto the tokens).
- `horcrux-api/horcrux-ui/src/components/{ui,shell}.rs`: Leptos components (state tones, badges, usage bars,
  gauges, panels, sortable grid headers, column chooser, row menus, step tabs, Appearance preferences,
  resource tree, header).
- `style/main.css`: legacy page styles, being folded into the system page by page.

Why the kit lives in this repo for now (and not in its own repo yet): the tokens and components are still
moving with the Horcrux migration, and one repo keeps a token change and every page it affects in one
reviewed commit. The kit has no Horcrux dependency (no Leptos, no fonts, no branding, only `ui-` classes and
CSS variables), so it can move to its own repo unchanged; Patronus vendors a pinned copy (`ui-kit.pin` =
commit + sha256 of the files) until then.

## What "polished" means here (measured against current consoles)

| Quality | Rule in this system |
|---|---|
| Information density | 13 px base type, 30 px table rows (24 px compact, 36 px comfortable), 16 px content padding. A 1280x800 window shows the whole datacenter summary without scrolling. |
| Persistent navigation | Left sidebar = live resource tree (nodes > guests > storage, with state dots) above grouped navigation. The header keeps search, Create VM/CT, live task and alert counts, and the user menu on every page. |
| One toolbar + grid pattern | Every list page is: page header (title, subtitle, primary action), toolbar (actions left, search/filter right), data grid (sortable headers, filter, column chooser, row count footer). |
| Colour = state only | Neutral chrome. Accent only for primary actions, selection and links. Green/amber/red/blue are reserved for ok/warning/critical/info and appear as dots, badges, usage bars and row tints. Zero is never red. |
| Live panels | Lists and summaries refresh on a timer (5-15 s) without re-rendering form state; the header shows the running-task count; the dashboard's Recent Tasks grid is live. |
| Wizards with steps | Multi-step creation uses `StepTabs` (numbered steps, current/done/todo states, Back/Next/Finish footer, a Summary step before Finish). |
| Inline help | Every non-obvious field has a hint line under it (`.form-help`) and a validation line that replaces it on error (`.form-error`). |
| Keyboard | `/` focuses global search, `Esc` closes dialogs/drawers, every control has a visible `:focus-visible` ring, tree and grid rows are real links (Tab, Enter, middle-click). |
| Empty / loading / error | `EmptyState` (what is missing + the action that fixes it), skeleton rows while loading, `424` node-lacks-feature states with the reason and the fix, inline error banners that keep the page usable. |

## Tokens

All tokens are CSS custom properties on `:root`, with a dark set applied by
`@media (prefers-color-scheme: dark)` (default: follow the device) or by `html[data-theme=dark|light]`
(the user's choice in the user menu, stored in `localStorage["hx.theme"]`). An inline script in
`index.html` applies the stored choice before first paint (no flash).

### Colour

| Token | Light | Dark | Use |
|---|---|---|---|
| `--c-canvas` | `#f3f4f6` | `#0f1216` | page background |
| `--c-surface` | `#ffffff` | `#161a20` | panels, grids, dialogs |
| `--c-sunken` | `#eceef1` | `#12161b` | sidebar, grid headers, toolbars |
| `--c-raised` | `#ffffff` | `#1c2128` | popovers, menus |
| `--c-border` | `#d9dde3` | `#2b323b` | 1 px rules |
| `--c-border-strong` | `#bcc3cc` | `#3a434e` | inputs |
| `--c-ink` | `#1a1f25` | `#e6e9ed` | text |
| `--c-muted` | `#5f6873` | `#98a2ad` | secondary text (>= 4.5:1 on canvas, surface and sunken) |
| `--c-chrome` | `#1b2027` | `#0b0e11` | header bar (dark in both schemes) |
| `--c-accent` | `#0b6e77` | `#5cc8cf` | links, selection, focus |
| `--c-accent-solid` | `#0b6e77` | `#137f88` | fill behind white text (primary buttons) |
| `--c-accent-subtle` | `#e0f1f2` | `#122a2d` | selected row / active nav item |
| `--c-brand` | `#c08a2e` | `#d9a54a` | the crest, never for state |
| `--c-ok` / `--c-ok-bg` | `#1d7a3e` / `#e5f4ea` | `#56c77a` / `#13261a` | running, healthy, online |
| `--c-warn` / `--c-warn-bg` / `--c-warn-fill` | `#8a5200` / `#fdf3dc` / `#f2b632` | `#f0b545` / `#2d2412` / `#d99a1e` | degraded, >= 75 % used |
| `--c-crit` / `--c-crit-bg` | `#b42318` / `#fdeceb` | `#ff7d73` / `#2e1715` | failed, offline, >= 90 % used |
| `--c-info` / `--c-info-bg` | `#1f5fb0` / `#e7f0fb` | `#79aefc` / `#14233a` | informational |

Every text/background pair above passes WCAG AA (4.5:1); the check lives in
`ci/ux-qa/check_ui_tokens.py` and runs in the `ux-qa-static` CI job.

Accent presets (`html[data-accent=verdigris|cobalt|garnet|graphite]`, user menu) swap only the
accent family; each preset is in the contrast check.

### Type

`--font-sans: "IBM Plex Sans"` (bundled, OFL, `assets/fonts/`), `--font-mono: "IBM Plex Mono"`;
system fallbacks after them. The UI works offline, so no font is fetched from the network.

| Token | Size / line | Use |
|---|---|---|
| `--fs-xs` | 11 / 16 | grid header labels, badge text |
| `--fs-sm` | 12 / 16 | meta text, hints, tree |
| `--fs-md` | 13 / 20 | body, grid cells, forms (base) |
| `--fs-lg` | 15 / 22 | panel titles |
| `--fs-xl` | 18 / 24 | page title |
| `--fs-2xl` | 24 / 30 | gauge values only |

Weights: 400 body, 500 labels/nav, 600 titles and key numbers. Numbers use `tabular-nums`.
IDs, MACs, paths, UPIDs and hashes use the mono face.

### Spacing, radii, elevation, density

- Spacing: 4 px scale `--s-1` 4, `--s-2` 8, `--s-3` 12, `--s-4` 16, `--s-5` 20, `--s-6` 24, `--s-8` 32.
- Radii: `--r-sm` 3 px (badges, inputs), `--r-md` 5 px (buttons, panels), `--r-lg` 8 px (dialogs).
- Elevation: `--e-0` none (panels sit on a 1 px border), `--e-1` menus/popovers, `--e-2` dialogs/toasts.
- Density (`html[data-density=compact|comfortable]`, user menu): `--row-h` 24/30/36 px, `--ctl-h` 26/30/34 px.
  Phones always get at least 44 px hit targets for buttons and nav items.

## Layout (app shell)

```
+--------------------------------------------------------------------------------+
| [crest] Horcrux 0.1.x   [ Search guests, nodes, storage   / ] [+VM][+CT] tasks alerts user |  44 px header
+----------------+---------------------------------------------------------------+
| RESOURCES      | Page header: title . subtitle                  [primary action] |
|  v node-1  (*) | Toolbar: [actions]                         [filter][columns]   |
|     100 web (*)| +-----------------------------------------------------------+ |
|     101 db  (o)| | data grid                                                  | |
|     local  ##  | +-----------------------------------------------------------+ |
| NAVIGATION     |                                                                |
|  > Compute     |                                                                |
|  > Storage ... |                                                                |
+----------------+---------------------------------------------------------------+
  232 px sidebar (sticky, own scroll)        content max 1600 px, 16 px padding
```

Phones (< 900 px): the sidebar becomes an off-canvas drawer behind the header's menu button;
header actions collapse to icons with labels kept for screen readers.

## Components

| Component | Where | Notes |
|---|---|---|
| `AppShell` (header, `Sidebar`, `ResourceTree`) | `lib.rs`, `components/shell.rs` | tree polls `/api/vms`, containers, nodes, storage every 15 s; state dots; current object highlighted; groups collapse (`<details>`, keyboard native) and remember their state |
| `PageHeader` | `components/ui.rs` | title (the page's h1), optional subtitle/breadcrumb, actions slot |
| `Panel` | `ui.rs` | titled section with header actions; `.panel-grid` lays panels in 2/3 columns |
| `Toolbar` | `.toolbar` | `.toolbar-start` actions, `.toolbar-end` search/filter; 36 px tall |
| `DataGrid<T>` | `ui.rs` | columns with sort key + renderer, text filter, column chooser (stored per grid in localStorage), row count footer, empty state, sticky header, row links |
| `Badge` / `StateDot` | `ui.rs` | `state_tone()` maps every state word the API uses (running, stopped, online, failed, degraded, ok, warning, critical...) to a tone |
| `UsageBar` / `Gauge` | `ui.rs` | thresholds 75 % warn / 90 % crit, text value always shown (never colour alone) |
| `StepTabs` | `ui.rs` | wizard steps with done/current/todo, used by Create VM/CT, storage and SAN wizards |
| `EmptyState` | `ui.rs` | title, one line of why, the fixing action |
| Tabs | `.tabs/.tab` | underline tabs, `aria-selected` |
| Forms | `.form-grid`, `.form-group`, `.form-help`, `.form-error` | label column 160 px on desktop, stacked on phones |
| Dialog | `.modal-overlay/.modal-content` | 560 px default, header/body/footer, Esc closes, focus trapped by the page |
| Toast | `.toast-stack` | bottom-right, auto-dismiss 6 s, errors stay until dismissed |
| Log viewer | `.log-view` | mono, priority tint per line (`.log-crit/.log-err/.log-warn/.log-notice/.log-info/.log-debug`), live toggle, reboot separators |

## Rules for new pages

1. Pick the surface first: list pages are Operate (toolbar + grid), the dashboard and node/VM summary
   are Monitor (panels, gauges, grids), creation is Configure (StepTabs wizard). No hero sections.
2. Use the components; a page-local style is allowed only for layout inside a panel, and only with tokens.
3. Every count is a link to the filtered list (ux_crawl `unlinked-summary`); zero is never red.
4. Every page has its own title and h1; every list has an empty state with an action.
5. Screenshots: `docs/qa/ui-shots/` holds the committed first-screen baseline of the key pages (dashboard,
   VM list and detail, storage, resources, tasks, health, logs, monitoring, users, containers, backup,
   network, HA) at desktop 1280 and phone 390, light and dark (`ci/ux-qa/ui_snapshots.py --write` from a
   `ux_crawl.py --shots` run). A UI change re-captures them in the same commit; the CI UX crawl step reports
   every page whose first screen changed by more than 25 %, so review those PNG pairs like code.
