# ui-kit/map v0.1: the shared infrastructure map component

Draws an `infra-map/v1` graph (docs/design/infra-map.md section 1) for Horcrux (`/map`) and Patronus
(`/map`, vendored with a checksum pin in `crates/patronus-web/ui-map/`). No dependencies, no build step,
budget `map.js` <= 24 KB, `map.css` <= 6 KB (`python3 ui-kit/map/check.py`, also runs `node test.js`).

```html
<link rel="stylesheet" href="/ui-map/map.css">
<script src="/ui-map/map.js"></script>
<div id="m"></div>
<script>
  const m = UiMap.mount(document.getElementById('m'), {
    src: '/api/map/graph',                 // any infra-map/v1 URL
    headers: {},                           // e.g. {'X-Map-Key': key} for a read-only share
    refresh: undefined,                    // seconds; default = the graph's refresh_secs, 0 = never
    readonly: false,                       // true: no "Open" links, no finding actions
    emptyTitle: 'No devices yet', emptyText: '...',
  });
  m.reload();                              // after a change
</script>
```

- Layout is deterministic (hubs = ungrouped router/host/internet nodes on top, one column per group,
  ungrouped others in "Other"): the same graph always gives the same picture.
- Toolbar: filter (name, IP, MAC, class), state filter, List view (keyboard and screen-reader table),
  zoom, Export PNG / SVG / Mermaid / JSON. Findings render as banners above the map.
- `UiMap.toMermaid(graph)` and `UiMap.layout(graph)` are pure and tested (`test.js`).
- Uses kit tokens (`--c-ink`, `--c-ok-solid`, ...) with fallbacks; every class starts with `ui-map` or
  styles a kit class inside `.ui-map`, so pages that have not adopted the kit yet still render it.

Changing it: edit here, run `python3 ui-kit/map/check.py`, then re-vendor into Patronus (copy `map.js`,
`map.css`, set `commit` + sha256 lines in `crates/patronus-web/ui-map/map.pin`).
