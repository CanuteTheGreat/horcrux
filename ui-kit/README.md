# ui-kit v0.2 (shared by Horcrux and Patronus)

Framework-neutral design system: plain CSS custom properties, `ui-`-prefixed component classes, and an
optional dependency-free `ui.js`. No fonts, no branding, no build step, no npm. The design rules (density,
colour = state only, live panels, keyboard) are in `docs/design/ui-system.md`.

| File | What | Budget (CI: `python3 ui-kit/check.py`) |
|---|---|---|
| `tokens.css` | colours (light + dark + accent presets), type scale, spacing, radii, elevation, density | with components.css <= 30 KB minified |
| `components.css` | every class starts with `ui-`; nothing styles bare elements, so pages adopt it one at a time | |
| `ui.js` | optional: grid sort/filter/column chooser, tabs, log follow (`data-ui-*` attributes) | <= 15 KB minified |
| `gallery.html` | every component, open it directly in a browser (light/dark follow the system) | |

`check.py` also fails on any text/background token pair under WCAG AA (4.5:1) in either palette.

## Theming contract

- No `data-theme` on `<html>` = follow the device (`prefers-color-scheme`); `data-theme="light|dark"` forces
  one. Set it before first paint from the product's own storage key (Horcrux: `hx.theme`).
- `data-accent="cobalt|garnet|graphite"` switches the accent; `data-density="compact|comfortable"` the row
  height (default 30 px rows).
- A product theme file overrides only brand tokens after `tokens.css`: `--c-brand`, `--c-accent*`,
  `--font-sans`, `--font-mono` (Horcrux opts into bundled IBM Plex there).

## Markup contract (one snippet per component)

```html
<!-- page header -->
<div class="ui-page-header"><div><h1>Virtual Machines</h1><p class="ui-sub">8 VMs · 3 running</p></div>
  <div class="ui-actions"><button class="ui-btn primary">Create VM</button></div></div>

<!-- tree: nest ul; a.active or aria-current="page" marks the current object -->
<div class="ui-tree-heading">Resources</div>
<ul class="ui-tree"><li><a href="/" class="active"><span class="ui-dot ok"></span>Datacenter</a>
  <ul><li><a href="/vms/vm-100"><span class="ui-dot off"></span>web</a></li></ul></li>
  <li><details open><summary>Storage</summary><ul><li><a href="/storage">Pools</a></li></ul></details></li></ul>

<!-- state: dot shape differs per tone (ok, warn, crit, info, busy, off) -->
<span class="ui-badge ok"><span class="ui-dot ok"></span>running</span>

<!-- toolbar + grid; data-ui-* only needed with ui.js -->
<div class="ui-toolbar"><div><input class="ui-input" type="search" data-ui-filter="vms" aria-label="Filter"></div>
  <div><details class="ui-menu" data-ui-columns="vms"><summary class="ui-btn sm">Columns</summary>
    <div class="ui-menu-items"><label><input type="checkbox" value="mem"> Memory</label></div></details></div></div>
<div class="ui-grid-wrap"><table class="ui-grid" data-ui-grid="vms">
  <thead><tr><th data-sort>Name</th><th data-sort data-type="num" class="num" data-col="mem">Memory</th></tr></thead>
  <tbody><tr><td>web</td><td class="num" data-col="mem">2 GiB</td></tr></tbody></table>
  <div class="ui-grid-foot"><span>1 of 1</span></div></div>
<div class="ui-grid-state">No VMs yet. <a href="/vms/create">Create VM</a></div>   <!-- empty; add .error for errors -->

<!-- panel, key/value, usage bar (add warn / crit) -->
<section class="ui-panel"><div class="ui-panel-head"><h2>Summary</h2><a href="/x">Details</a></div>
  <div class="ui-panel-body"><dl class="ui-kv"><dt>CPU</dt><dd>
    <div class="ui-usage warn"><span class="track"><i style="width:81%"></i></span><span class="val">81%</span></div></dd></dl></div></section>

<!-- tabs (ui.js: arrow keys, panels by aria-controls) -->
<div class="ui-tabs" data-ui-tabs role="tablist"><button role="tab" aria-selected="true" aria-controls="p1">Summary</button></div>
<div id="p1" role="tabpanel">...</div>

<!-- wizard steps -->
<ol class="ui-steps"><li class="done">General</li><li aria-current="step">Disks</li><li>Confirm</li></ol>

<!-- form field: hint under the control; .ui-error + aria-invalid on error; .ui-form-grid = label column -->
<div class="ui-form-grid"><div class="ui-field"><label for="n">Name</label><input id="n" class="ui-input">
  <span class="ui-hint">Letters, digits and dashes.</span></div></div>

<!-- banner (ok, info, warn, crit), empty state, loading -->
<div class="ui-banner warn">Two guests are not managed. <button class="ui-btn sm">Import</button></div>
<div class="ui-empty"><strong>No backups yet</strong>Back up a VM to see it here. <a class="ui-btn" href="/backup">Back up now</a></div>
<p class="ui-loading">Loading…</p>

<!-- dialog (native <dialog>), toast -->
<dialog class="ui-dialog"><header>Delete VM</header><div class="ui-dialog-body">...</div>
  <footer><button class="ui-btn">Cancel</button><button class="ui-btn danger solid">Delete</button></footer></dialog>
<!-- the parts may also sit in one <form> directly inside the dialog (Enter submits, `required` validates):
     <dialog class="ui-dialog"><form method="dialog"><header>..</header><div class="ui-dialog-body">..</div><footer>..</footer></form></dialog>
     .ui-dialog sets margin:auto itself, so a page-level `* { margin: 0 }` reset doesn't pin it top-left. -->
<div class="ui-toasts"><div class="ui-toast ok">VM started.</div></div>

<!-- log viewer: p-crit/p-err/p-warn/p-notice/p-debug rows, .boot separators; data-ui-log follows the end -->
<div class="ui-log" data-ui-log><div class="boot">-- boot 3f2a --</div>
  <div class="p-warn"><span class="ts">14:03:40</span>pool tank is 81% full</div></div>
```

## Changes

- v0.2: `ui-dialog` header/body/footer also match inside a direct child `<form>`; `.ui-dialog` sets `margin: auto`.

## Stability (v0.2)

Stable: tokens, `ui-btn`, `ui-badge`, `ui-dot`, `ui-panel`, `ui-kv`, `ui-toolbar`, `ui-grid`, `ui-tabs`,
`ui-steps`, `ui-field`, `ui-banner`, `ui-empty`, `ui-log`, `ui-usage`. May still change: `ui-tree` nesting
depth styles, `ui-dialog`, `ui-toast`, `ui-menu`, charts (not in v0.2).
