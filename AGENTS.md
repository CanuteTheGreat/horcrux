# Horcrux -- Instructions for AI Coding Agents

## Platform: Gentoo Linux, from the ground up

Horcrux is built specifically for Gentoo Linux. This is not incidental --
it is the core design point of the project, alongside Portage's USE-flag
model (fine-grained feature selection at build time). Do NOT default to
generic/Debian/Ubuntu conventions just because they are common boilerplate
for a Rust project. Specifically:

- The canonical install path is the real Gentoo ebuild + overlay at
  `gentoo/app-emulation/horcrux/` (metadata.xml, USE flags, init scripts,
  package.use, package.accept_keywords all live there). Any packaging or
  install-flow change belongs there first.
- `docker-compose.yml` / `Dockerfile` build through real Portage (`emerge`
  against that same overlay), NOT `cargo build` directly against a generic
  Debian/Ubuntu base image. If you ever see a Dockerfile in this repo doing
  `FROM rust:*` -> `cargo build` -> `FROM debian:*-slim`, that is WRONG and
  a regression -- it was fixed once already (2026-09-24) after being
  silently introduced by an earlier AI coding session that defaulted to
  boilerplate instead of checking the project's actual target platform.
- USE flags (see `gentoo/app-emulation/horcrux/horcrux-0.1.0.ebuild`'s
  `IUSE`) are the actual selling point over competitors like Proxmox --
  they let an installer choose exactly which virtualization backends,
  container runtimes, and NAS protocols get compiled in. Any container or
  CI build path MUST preserve this (pass USE flags through as a build arg
  to `emerge`), not silently compile a fixed feature set.
- Any new documentation, README section, CI workflow, or install script
  should assume Gentoo as the primary/default target. Non-Gentoo Linux can
  be mentioned as "also runs on any modern distro" but must never replace
  or overshadow the Gentoo-first framing.

If you are an AI coding agent working on this repo and are unsure whether
something should be Gentoo-specific, ask -- do not assume "generic Linux
container" is a safe default here. It has caused real, silent architectural
drift before.

## Access control: every route is permission-checked

Protected routes run through `middleware/rbac.rs`, which asks
`crates/horcrux-auth/src/auth/privileges.rs::requirement(method, path)` what the
request needs (a Proxmox-style privilege on an ACL path). When you add a
route under a new top-level `/api/<area>`, add a rule for it there - CI's
`every_registered_route_is_mapped` test fails otherwise, and an unmapped
area is administrators-only. Tests must create users through the admin API
(`POST /api/users` + `POST /api/acl`): public registration is disabled. The
API serves HTTPS (self-issued certificate): test clients use
`https://localhost:8006` with `danger_accept_invalid_certs(true)` (or the
node CA from `/api/tls/ca.pem`). See docs/ACCESS_CONTROL.md.

## Code layout: a workspace of crates (keep CI build memory low)

`horcrux-api` used to be one 130k-line crate whose `main.rs` re-declared
every module, so CI compiled it twice and needed 9+ GB. It is now split
along its module seams into `crates/`:

| crate | modules |
|---|---|
| `horcrux-core` | error, config, features (USE-flag table), validation, encryption, secrets, logging, db |
| `horcrux-auth` | auth (realms, RBAC privileges), middleware, audit, tls, websocket |
| `horcrux-monitoring` | metrics, monitoring, alerts, notification_channels, observability, prometheus, webhooks |
| `horcrux-storage` | storage, template, cloudinit |
| `horcrux-sdn` | sdn, firewall |
| `horcrux-vm` | vm, console, gpu |
| `horcrux-container` | container (docker/podman/lxc/lxd/incus) |
| `horcrux-cluster` | cluster, ha, migration |
| `horcrux-backup` | backup (only built with USE=backup) |
| `horcrux-k8s` | kubernetes (only built with USE=kubernetes) |
| `horcrux-nas` | nas (only built with USE=nas) |

`horcrux-api` keeps the HTTP layer: `src/app/` (startup, router, the
handlers that used to be in main.rs), `src/api/<lane>.rs` route modules,
`state.rs` (AppState); `main.rs` only calls `horcrux_api::app::run()`.
Every module is still reachable as `crate::<module>` / `horcrux_api::<module>`
(each crate re-exports the modules it depends on), so moved code needs no
path changes. Rules:

- Every USE flag stays a Cargo feature of `horcrux-api`; it forwards the
  feature to each crate whose code is gated on it (`qemu = [..., "horcrux-vm/qemu"]`).
  A `#[cfg(feature = "x")]` inside a crate needs `x` declared in that
  crate's Cargo.toml and forwarded from horcrux-api.
- Dependencies are declared once in the root `[workspace.dependencies]`.
- New code goes in the crate that owns the module; only HTTP handlers and
  wiring belong in horcrux-api. Don't add a dependency from a lower crate
  to a higher one (core <- auth/monitoring/storage/sdn/vm <- container,
  cluster <- backup <- api); move the shared type down instead.

