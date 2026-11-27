# Infrastructure map, discovery and shared live view (Horcrux + Patronus)

> Shared design, kept identical in both repos (Horcrux `docs/design/infra-map.md`, Patronus
> `docs/design/network-map.md`). The map component's source of truth is Horcrux `ui-kit/map/`;
> Patronus vendors it in `crates/patronus-web/ui-map/` (pinned, `ci/check-ui-map-pin.py`).

Status: design + MVP (2026-11-27). Prior art studied for ideas only (no code copied): HomeLabel (MIT:
nmap discovery, Proxmox/UniFi/Unraid import, live VM/device map, per-node up/down, rack designer, PNG +
Mermaid export, read-only key-protected live view, Homepage widget), NetBox (Apache-2.0: source of
truth, racks/cables), Proxmox VE (resource tree only, no map), pfSense/OPNsense (flat ARP/DHCP lists, no
map), UniFi (topology view of its own gear only).

The bar: neither Proxmox nor pfSense/OPNsense draws a map. Horcrux draws compute, Patronus draws the
network, and Horcrux can ingest Patronus so one page shows the whole home lab / office, built from what the
products already manage. No hand-drawn diagrams, nothing to keep in sync.

## 1. One graph format: `infra-map/v1`

Both products emit the same JSON, so the same component draws both, Horcrux can merge a Patronus map, and
exports are identical.

```json
{
  "schema": "infra-map/v1",
  "source": {"product": "patronus", "name": "router-1", "generated_at": 1764260000},
  "refresh_secs": 30,
  "groups": [{"id": "seg:@lan", "label": "LAN", "kind": "segment", "sub": "192.168.1.0/24"}],
  "nodes": [{"id": "dev:aa:bb:cc:00:00:01", "label": "printer-1", "kind": "device", "class": "printer",
             "group": "seg:@lan", "status": "up", "ip": "192.168.1.20", "mac": "aa:bb:cc:00:00:01",
             "href": "/map#dev:aa:bb:cc:00:00:01", "meta": {"first seen": "..."}}],
  "edges": [{"from": "router", "to": "seg:@lan", "kind": "link", "label": "br0"}],
  "findings": [{"id": "flat-network", "severity": "high", "title": "...", "detail": "...", "nodes": ["..."]}],
  "counts": {"nodes": 12, "up": 10, "down": 2}
}
```

- `status`: `up | down | warn | unknown` (+ product words mapped into these: running -> up, stopped -> down).
- `kind` (open set): `router host vm container storage bridge segment device switch ap internet external`.
- `class` (devices): `computer phone printer camera iot media infra unknown`; user-overridable.
- Node ids are namespaced (`dev:`, `vm:`, `ct:`, `st:`, `br:`, `node:`); a merged source prefixes its own
  ids with `<source>/` so two graphs never collide.

## 2. Shared UI component (build once): `ui-kit/map/`

Lives next to the shared ui-kit in the Horcrux repo (upstream), vendored by Patronus with a checksum pin
(`crates/patronus-web/ui-map/map.pin`, `ci/check-ui-map-pin.py`), exactly like the kit itself. It only uses
kit tokens (`--c-*`) with fallbacks, so it works before and after a page adopts the kit.

- `map.js` (no dependencies, no build step, budget 24 KB): `UiMap.mount(el, {src, headers, refresh})`
  fetches the graph, lays it out deterministically (groups = columns, hubs on top, no physics: the same data
  always gives the same picture, which also makes visual snapshots stable), draws SVG with status dots and
  edges, a filter box, status/class filters, zoom, a node detail panel and a keyboard/screen-reader list view
  (every node is a focusable link; the list view is the a11y and phone fallback).
- Export: PNG (SVG rasterised in the browser, colours inlined), SVG, Mermaid (`flowchart LR`, a subgraph
  per group) and the raw JSON. `UiMap.toMermaid(graph)` is a pure function with its own test
  (`ui-kit/map/test.js`, run by `ui-kit/map/check.py` in CI).
- Live: re-fetches every `refresh_secs` (server value, overridable), pauses while the tab is hidden.
- `map.css` (budget 6 KB): `ui-map-*` classes only.

## 3. Read-only live view, share keys, dashboard widget (both products)

- Share keys: an admin creates a named key (optional expiry, scope `view` or `stats`). Only its SHA-256 is
  stored; the key (`pmk_...` Patronus, `hmk_...` Horcrux, 32 random bytes base64url) is shown once.
- Live view page `/map/live#k=<key>` needs no login and shows the map read-only. The key sits in the URL
  fragment, which browsers never send to the server or proxy logs; the page passes it as `X-Map-Key`.
- `GET .../shared/graph` (scope view) and `GET .../stats` (scope stats or view) accept `X-Map-Key` (or
  `?key=` for tools that cannot set headers). A shared graph drops MAC addresses and `meta` unless the key
  was created with `detail: full`.
- Stats (flat JSON, Homepage `customapi` widget compatible):
  `{"devices": 24, "up": 21, "down": 3, "segments": 3, "findings": 1}` (Patronus) /
  `{"hosts": 1, "vms": 8, "vms_running": 5, "containers": 4, "storage": 2, "down": 3}` (Horcrux).
  Homepage example in each product's docs.
- Keys are listed (name, scope, created, last used, expiry) and revoked from the map page.

## 4. Patronus (the router): device discovery, network map, segmentation advisor

Data already there: DHCP leases (Kea/dnsmasq, `patronus_network::dhcp::leases`), the kernel neighbour table
(`ip neigh`, already parsed for segmentation), interfaces/roles/VLANs/subnets (`seg::segment_v4`), enrolled
segmentation devices and groups. New:

- Discovery loop (in patronus-web, like the insight collector): every `map.discovery_interval_secs` (60)
  merge leases + neighbour table (+ optional probe) into `map_devices` (measured, `NON_CONFIG_TABLES`,
  retention `map.retention_days` = 30). Up = seen REACHABLE/answering within `map.down_after_secs` (300).
  Probe `map.probe`: `off` | `ping` (default: one ICMP echo to each address already known; never a
  sweep; phones that ignore ICMP stay up while the kernel sees them REACHABLE) | `nmap` (opt-in `nmap -sn` sweep of each LAN-side subnet, only when the
  binary exists; refused on WAN).
- LLDP: when `lldpctl` (net-misc/lldpd) is installed, its JSON neighbours become switch/AP nodes on the
  router port they were heard on (with chassis name and port). Device-to-switch-port placement needs the
  switch's own FDB (SNMP): later.
- Classification: user label (`map_device_labels`: name, class, note; configuration, exported) > user rules
  (`map_class_rules`: hostname/MAC-prefix glob -> class) > built-in hostname patterns > `unknown`.
  Locally administered MACs are marked "private address" (phones/laptops randomise).
- Map: router hub -> one group per LAN-side segment (role/VLAN + subnet) -> devices; WAN uplinks -> an
  internet node; LLDP neighbours on their port. Page `/map` (Network > Map), API `GET /api/map/graph`,
  `GET /api/map/devices`, `PUT /api/map/devices/<mac>` (label/class), `/api/map/rules`, `/api/map/keys`,
  `GET /api/map/stats`, public `/map/live` + `/api/map/shared/graph`.
- Segmentation advisor (security; ties to the owner pentest finding "flat home LAN"): per segment, if
  untrusted classes (`iot camera printer media`, configurable `map.advisor.untrusted`) share a segment with
  trusted/sensitive ones (`computer phone infra`) and nothing isolates them (segmentation off, or everyone
  in one group), raise `flat-network` (high when cameras/IoT sit next to infra consoles). The fix reuses
  the device segmentation engine (own nft table `inet patronus_seg`, no new firewall code):
  `POST /api/segmentation/advisor/apply` = in ONE snapshot: switch isolation on for the segment (adopts
  everyone into `home`), create the groups the plan needs (`iot`, `cameras`, `printers`, `media`; names,
  internet access and intra-group traffic configurable), move each classified device, add one-way
  services (home -> printers 631/9100, home -> cameras 80/443/554) and a home<->media link (casting), then
  run the appliers; failure restores the snapshot. `dry_run: true` returns the plan and the nft preview.
  Undo = config revision rollback (System > History). A VLAN plan (IDs from `map.advisor.vlan_base`) is
  shown for networks with VLAN-capable switches/APs; applying VLANs is a later step.
- Optional IoT inventory (Zigbee2MQTT, Z-Wave JS, Home Assistant): pulled read-only into the same
  inventory as child nodes of their coordinator: later (spec: `map_iot_sources`, token in the store,
  Zigbee2MQTT over MQTT `bridge/devices`, HA `/api/states` with a long-lived token).

Tiers: standard tier and containers get it (USE=netmap, default on). Router-tier images (AX1800, Slate)
build an explicit feature list; adding it there waits for a measured RAM/flash cost (later).

## 5. Horcrux: compute map, imports, Patronus ingest

Data already there: `/api/tree` inventory (VMs, containers, storage, pools; RBAC-filtered), VM NICs
(bridge, VLAN, MAC), VM disks (paths), cluster nodes with status. New:

- `GET /api/infra-map`: one group per cluster node (host hub with status), VMs/containers/storage inside,
  bridges as nodes with VM->bridge edges (VLAN label), VM->storage edges (disk path under the pool),
  plus every configured source merged in. Same RBAC filter as the tree.
- Sources `GET/POST/DELETE /api/infra-map/sources` (`kind`: `patronus` | `proxmox` | `unifi` | `unraid`;
  `url`, secret, `verify_tls`, `refresh_secs`). MVP: `patronus` (its share key; fetches `infra-map/v1`
  live) and `proxmox` (read-only API token `PVEAPIToken=user@realm!id=secret`, `GET
  /api2/json/cluster/resources`: nodes, qemu, lxc, storage become groups/nodes; nothing is written to
  Proxmox). UniFi (`/proxy/network/api/s/<site>/stat/device` + `stat/sta`) and Unraid (GraphQL API
  key) are specified; adding one answers 424 `infra-map-source-not-built` until they are implemented.
- Cross-product link: a VM NIC MAC that Patronus also sees becomes an edge `same` between the Horcrux VM and
  the Patronus device, and the Patronus segment it sits in, so compute and network meet on one map.
- Page `/map` (Datacenter > Infrastructure map), share keys/live view/stats as in section 3.
- Rack designer (physical units, devices with U height, patch cables between ports, exported with the
  map): later, after the maps; spec: `rack`, `rack_item{rack,u,height,label,node_ref}`, `cable{a_port,
  b_port,label,colour}`; drawn by the same component as a second view.

## 6. Security and performance

- Live view and stats are read-only by construction (separate handlers that only read), keys hashed, no
  session created, expiry, revocation, last-used recorded; shared graphs omit MACs/meta by default.
- Horcrux source secrets are stored like metric-server tokens: never returned by the API (`********`),
  sending the mask back keeps the stored one. Outbound fetches have a timeout (`timeout_secs` 10), a size
  cap (4 MiB) and a per-source cache (`refresh_secs`); TLS verification is on by default.
- Discovery is passive by default; `ping` only probes known addresses; `nmap` sweeps are opt-in and LAN-only.
- Cost: one 60 s merge of tables the router already holds; graph JSON for 250 devices ~60 KB.

## 7. MVP vs later

| | MVP (this change) | Later |
|---|---|---|
| Shared | map.js/css, PNG/SVG/Mermaid/JSON export, live view, keys, stats | rack view, saved layouts |
| Patronus | discovery (leases, neigh, ping/nmap probe, LLDP), map page, classes + rules + labels, flat-network advisor + one-click isolation apply, Homepage stats | VLAN one-click apply, switch FDB/SNMP port placement, IoT pull, router-tier images |
| Horcrux | compute map, Patronus ingest + MAC correlation, Proxmox import | UniFi, Unraid, rack designer |

Proof: Patronus netlab lane `ci/netlab/lane-w-netmap.sh` (claim C1700) (fake devices in namespaces on a flat LAN, advisor flags it,
apply isolates with real nft state and real traffic), Horcrux unit tests + the live-server suite `horcrux-api/tests/it/infra_map_tests.rs` (fake Patronus source), browser
screenshots of both maps, `ui-kit/map/test.js`.
