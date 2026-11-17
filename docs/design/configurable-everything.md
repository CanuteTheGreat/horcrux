# Configurable everything: no hard-coded operational values

Owner rule (2026-11-17): *"honestly nothing should be hard-coded. everything should be
configurable and definable (like nix)"*.

This document is the plan and the ledger for Horcrux. The machine-generated, line-by-line
inventory is [configurable-everything-inventory.md](configurable-everything-inventory.md);
the gate that keeps new hard-coded values out is `ci/hardcode-scan.py` (job
`public-leak-guard`, step "No new hard-coded operational values"). The same scanner runs in
Patronus.

## The rule, concretely

An *operational value* is anything an owner could reasonably want different: host and device
names, addresses and subnets, ports, interface names, product data locations, timeouts,
intervals, retry counts, size limits, user/group ids, fixed lists (time zones, locales,
image catalogs), URLs of update servers and catalogs, feature defaults baked into an image.

Every such value is a **typed key with a documented default and bounds**, validated at load,
**settable** where a user would touch it, and part of the configuration that is exported,
imported and versioned. Images ship defaults, never owner values (owner-specific values
live only in the private overlay).

Two places hold Horcrux configuration, and every key lives in exactly one:

1. **Node configuration** (`HorcruxConfig`, `/etc/horcrux/config.toml`, env overrides
   `HORCRUX_*`): how this node's services behave (ports, paths, login lifetimes, limits).
   Typed serde structs with `#[serde(default = ...)]` defaults and `validate()` at start;
   documented in `horcrux-api/config.example.toml`. Test
   `config::tests::every_config_key_round_trips_through_toml` writes a non-default config
   and reads it back (every key survives; omitted keys get their documented defaults), and
   `shipped_example_config_parses_and_validates` keeps the example honest.
2. **Desired state** (declarative spec: VMs, pools, SDN, containers, users, firewall,
   backups; `docs/design/declarative-config.md`, export/plan/apply/GitOps/history). The
   spec is the export format, so its round trip is the engine's own contract (export ->
   plan shows no change), owned by the declarative-config lanes.

What stays a constant: protocol numbers and well-known protocol ports, special-purpose
address ranges, crypto/format sizes, OS/package layout (`/proc`, where a distro daemon's own
init script reads its config), test fixtures and UI example text. A constant that only looks
operational carries `hardcode-ok: <reason>` (or a `hardcode-ok-begin:` / `hardcode-ok-end`
block).

## The gate (`ci/hardcode-scan.py`)

- Scope (`ci/hardcode-scan.json`): `horcrux-api` (incl. the UI), `horcrux-cli`,
  `horcrux-common`, every crate under `crates/`, the live installer and ISO inputs
  (`build/live-iso`, `build/installer`, `build/catalyst`), `deploy/`, packaged helper and
  init files under `gentoo/*/*/files/`.
- Categories: address, port, timing, constant (named `*_TIMEOUT|*_INTERVAL|*_MAX|...`),
  interface, wifi, hostname, url, path (product data/state locations and storage roots),
  uid/gid, size (limits), fixed-list (time zones, locales).
- Known hits are fingerprinted in `ci/hardcode-baseline.tsv` (line-independent, with a
  proposed key). A NEW hit that is not a typed config key, not a recognised constant and not
  marked `hardcode-ok: <reason>` fails CI. `--self-test` checks the classifier first;
  `--summary`, `--inventory FILE` and `--update` (after a conversion batch) are the tools.

## Inventory (at the first batch)

| category | must become config | legit constant | test fixture | total |
|---|---|---|---|---|
| address | 27 | 99 | 497 | 623 |
| constant | 56 | 8 | 2 | 66 |
| fixed-list | 10 | 6 | 14 | 30 |
| hostname | 0 | 1 | 0 | 1 |
| interface | 12 | 6 | 66 | 84 |
| path | 70 | 121 | 214 | 405 |
| port | 20 | 3 | 16 | 39 |
| size | 18 | 0 | 2 | 20 |
| timing | 251 | 4 | 93 | 348 |
| url | 58 | 52 | 35 | 145 |
| **all** | 522 | 300 | 939 | 1761 |

## Converted

| was | key | default | where settable |
|---|---|---|---|
| `USER_TOKEN_TTL_SECS = 86400` (horcrux-auth middleware) | `[auth] session_ttl_secs` / `HORCRUX_SESSION_TTL_SECS` | 86400 s (300 .. 30 d) | config.toml, env |
| `TFA_SETUP_TOKEN_TTL_SECS = 900` (horcrux-auth middleware) | `[auth] tfa_setup_ttl_secs` / `HORCRUX_TFA_SETUP_TTL_SECS` | 900 s (60 .. 3600) | config.toml, env |
| fixed 13-entry language list in the live installer (`liveinst/system.py` `LOCALES`) | the image's glibc `/usr/share/i18n/SUPPORTED` (every UTF-8 locale, `locale-gen` runs for the choice) | shortlist shown first; whole list only without glibc data | installer |

Already dynamic before this sweep: the System > Configuration page reads time zones and
locales from the node (zoneinfo, `locale -a`), and the installer's time-zone list comes from
`zone1970.tab`.

## Remaining, in priority order

P1 = owner-visible or security, P2 = operator tuning, P3 = rare.

1. **P1 a node settings page**: `HorcruxConfig` is file + env only. Expose the
   non-bootstrap keys (`[auth]` lifetimes and groups, limits below) through
   `GET/PUT /api/system/node-config` with the same validation, an Advanced settings page,
   and include them in the declarative spec as a `node_settings` kind (registered through the
   engine's `Kind` extension point once the declarative lanes have merged), so GitOps and
   history carry them.
2. **P1 catalog and update URLs**: LXC image catalog `DEFAULT_IMAGES_URL` and `CATALOG_TTL`
   (horcrux-container lxc_template.rs), ACME directory URLs (horcrux-auth tls/acme.rs; the
   CA should be selectable, not only Let's Encrypt), Kubernetes manifests/URLs in
   horcrux-k8s, overlay/binhost URLs in `deploy/gentoo`.
3. **P1 HA and fencing timings** (horcrux-api ha_loop.rs `WATCHDOG_SECS`,
   `KILL_BUDGET_SECS`, `DRIFT_SECS`, `ATTEST_*`, `SYNC_TIMEOUT_SECS`; ha_fence_agent.rs
   `AGENT_SETTLE_SECS`, `FENCE_*`): `[ha]` keys with bounds; the safety relations between
   them (kill budget < watchdog) validated together.
4. **P1 cluster ports** (horcrux-cluster migration.rs `MGMT_PORT`, `SSH_PORT`;
   membership.rs 8443): derive from `[server] port` and a `[cluster]` section.
5. **P2 monitoring and metrics** (`NODE_METRICS_INTERVAL_SECS`, `VM_METRICS_INTERVAL_SECS`,
   metrics explorer `SAMPLE_INTERVAL_SECS`/`RETENTION_SECS`, alert `EVAL_INTERVAL_SECS`,
   webhook retry/backoff caps, `MAX_SILENCE_SECS`): `[monitoring]` keys.
6. **P2 container/VM timings** (LXC `DEFAULT_SHUTDOWN_TIMEOUT`,
   `SHUTDOWN_SIGNAL_EVERY_SECS`, template retry delays, SPICE relay `IDLE_SECS`, QEMU
   start/stop waits in horcrux-vm, websocket heartbeat), rate-limit windows in
   horcrux-auth: per-area config sections.
7. **P2 storage locations** (horcrux-nas: share roots, `/srv`, `/mnt` defaults; backup
   `CHUNK_DIR`; audit-log path and rotation size in horcrux-auth audit/rotation.rs):
   derive from `[paths]` (`data_dir`, `log_dir`, `audit_log` already exist there).
8. **P2 size limits** (`GATE_BODY_LIMIT`, `MAX_OVF_BYTES`, rbac body cap, audit buffer
   caps): `[limits]` keys.
9. **P3 UI polling intervals** (horcrux-ui `set_interval` periods): read once from
   `/api/system/node-config` with today's values as defaults.
10. **P3 legacy text installer** (`build/installer/horcrux-installer.sh`: fixed time-zone
    menu, `eth0`/example addresses): either retire it in favour of the live installer or
    read zones from `zone1970.tab` like the live installer does.

Counts and the exact lines are in the generated inventory; `ci/hardcode-baseline.tsv` is
the work list and shrinks with each batch.
