# Horcrux vs Portainer: Container Management Comparison

This document tracks Horcrux's Docker/Podman container management against
[Portainer](https://www.portainer.io/)'s self-hosted ("Business"/"CE
single-environment") feature set, mirroring the format of
`COMPARISON.md`/`PROXMOX_COMPARISON.md`.

Horcrux is a **single-host, self-hosted Gentoo virtualization platform**,
not a multi-cloud/multi-environment SaaS control plane. The scope below
reflects that: everything Portainer offers for managing *one Docker/Podman
host's* containers is covered; Portainer's multi-environment/cloud-fleet
management features are explicitly out of scope (see the bottom section).

## Coverage summary

| Area | Portainer (CE, single env) | Horcrux |
|---|---|---|
| Container lifecycle (create/start/stop/pause/resume/delete) | ✅ | ✅ (`container/docker.rs`, `container/podman.rs`) |
| Container clone | ➖ (not a Portainer feature) | ✅ (`clone_container`) |
| Container stats/inspect | ✅ | ✅ (bollard-backed, `get_container_stats_api`, `inspect_container_api`) |
| One-shot `exec` (run command, get output) | ✅ | ✅ (pre-existing `exec_command`) |
| **Interactive exec/web terminal** (`docker exec -it`) | ✅ | ✅ Docker: real bidirectional PTY stream via bollard `create_exec`/`start_exec(tty: true)` over a ticket-authenticated WebSocket. Podman/CLI-fallback: `podman exec -i` over plain pipes - **no PTY resize or raw terminal mode**, documented gap (see below). |
| **Live log streaming** | ✅ | ✅ Docker: bollard `logs(follow: true)` streamed over WebSocket. Podman: `podman logs -f` piped, one-way (no stdin, same as `docker logs -f` has no stdin either - not a real gap). |
| **Image management** (list/pull/remove/inspect/tag) | ✅ | ✅ Docker via bollard (`bollard::image::*`), Podman via CLI (`container/image.rs`) |
| Pull progress | ✅ (per-layer progress bar) | ✅ Docker: per-layer `CreateImageOptions` progress collected and returned; UI shows pull completion. Live per-layer progress bar in the UI is not implemented (progress data is available via the API, just not animated client-side yet). |
| **Volume management** (list/create/remove/inspect) | ✅ | ✅ Docker via bollard, Podman via CLI (`container/volume.rs`) |
| **Container-runtime network management** (list/create/remove/inspect/connect/disconnect) | ✅ | ✅ Docker via bollard, Podman via CLI (`container/cnetwork.rs`) - explicitly scoped to `docker network`/`podman network` bridge networks, **not** Horcrux's own SDN/CNI VM networking, which is a separate pre-existing subsystem (`sdn/cni`, `/api/cni/*`) |
| **Compose / "stack" deployment** | ✅ (Compose + Swarm stacks) | ✅ Compose-only, single-host. Parses `services:` (`image`, `command`, `environment`, `ports`, `volumes`, `depends_on`), creates+starts one container per service in dependency order, and tracks the group as a named "stack" persisted in SQLite (`container_stacks`/`container_stack_services`, migration 020). See "Compose scope" below for what's intentionally not applied. |
| **Live events feed** | ✅ | ✅ Docker: bollard `events()` stream over WebSocket. Podman: `podman events --format json --stream` piped over the same WebSocket contract. |
| Templates / App Store | ✅ | ❌ Not implemented (tracked as a possible future addition, not attempted here) |
| Registry management (private registries, credentials) | ✅ | ❌ Not implemented |
| RBAC / teams scoped to container resources | ✅ | ➖ Horcrux has its own global RBAC (`auth`/`middleware::auth`) but no per-container-resource ACLs yet |
| Webhooks (image update triggers) | ✅ | ➖ Horcrux already has a generic webhook subsystem (`webhooks/`) unrelated to this work; not wired to image pulls specifically |

## Interactive exec: documented Podman/CLI-fallback gap

Docker's interactive exec (`ContainerWsKind::Exec` + bollard) gets a real
attached PTY (`tty: true` on both `create_exec` and `start_exec`), matching
what `docker exec -it` does and what Portainer's web terminal gives you.

Podman (and Docker when the bollard API socket isn't reachable, which falls
back to the same code path) instead shells out to `<runtime> exec -i
<container> <cmd>` with plain stdin/stdout pipes. This **works** - you can
run commands and see output over the WebSocket - but it does **not**:

- allocate a pseudo-TTY, so curses-style programs (`top`, `vim`, interactive
  shells with line editing) will behave like a dumb pipe, not a terminal
- support `docker exec`-style terminal resize (`ResizeExecOptions` has no
  CLI equivalent being wired through yet)
- forward job-control signals (Ctrl-C) the way a real PTY does

This is called out explicitly in `container/ws_handlers.rs` rather than
being silently passed off as equivalent to the Docker path. Closing this
gap for Podman would mean either running `podman exec -it` under a real PTY
allocation (e.g. via a `pty`-wrapping crate) or waiting for broader podman
REST API exec/attach support to be assumed-present on target hosts - out of
scope for this pass.

## Compose/stack: explicit scope

**Supported** compose keys (applied per service):
`image`, `command`, `environment`, `ports`, `volumes`, `depends_on`
(used for deploy ordering only - a topological sort, not health-check
gating).

**Recognized but intentionally unsupported** (flagged back to the caller in
`unsupported_features` rather than silently ignored):

- `build:` - Horcrux has no image-build pipeline; provide a pre-built
  `image:` instead.
- `deploy:` (replicas, placement, update_config, rollback_config) - Swarm
  orchestration concept; Horcrux is single-host, see "Out of scope" below.
- `configs:` / `secrets:` - Swarm-only distributed config/secret objects.
- `healthcheck:` - parsed but not yet translated into a running healthcheck.

A "stack" in Horcrux is **not** a Swarm service: it's a named grouping of
plain containers, each created via the existing `ContainerManager`
(same create/start path as a manually-created container), with the
group membership persisted in two new SQLite tables
(`container_stacks`, `container_stack_services`) via the same
`sqlx`-backed `Database`/migrations layer every other persistent subsystem
(VMs, users, NAS scheduler, etc.) already uses - there's no separate
storage mechanism invented for this feature.

## Out of scope (deliberately, not an oversight)

These Portainer features are **not** implemented and are not planned as
part of this work, because they solve problems Horcrux's single-host,
self-hosted-per-Gentoo-node architecture doesn't have:

- **Docker Swarm support** (service/stack orchestration across a Swarm
  cluster, rolling updates, placement constraints). Horcrux's own
  clustering/HA story (`ha/`, `cluster_manager`) is VM/node-level, and
  container "stacks" here are intentionally a lightweight single-host
  grouping, not a Swarm scheduler.
- **Azure Container Instances (ACI) environment type.** Horcrux doesn't
  manage cloud-provider container services; it manages containers on the
  Gentoo host it's installed on.
- **Edge Agent / Edge Compute environments** (Portainer's model for
  managing many remote Docker hosts behind NAT through a polling agent).
  Horcrux is deployed per-node; there is no fleet-of-remote-Docker-hosts
  concept to manage centrally.
- **Multi-environment / multi-cloud management** (Portainer's defining
  feature: one UI across many Docker/Kubernetes/ACI environments).
  Horcrux already has its own multi-node clustering model for VMs; this
  work only extends the existing per-node `ContainerManager`, it does not
  attempt to turn Horcrux into a Portainer-style fleet manager for
  third-party Docker hosts it doesn't otherwise manage.
- **Container templates / App Store** and **private registry credential
  management** - genuinely useful, simply not attempted in this pass;
  listed above as real gaps rather than being pretended away.

## Where the code lives

- `horcrux-api/src/container/image.rs` - image management
- `horcrux-api/src/container/volume.rs` - volume management
- `horcrux-api/src/container/cnetwork.rs` - container-runtime network management
- `horcrux-api/src/container/stack.rs` - compose parsing + stack deploy/list/start/stop/remove
- `horcrux-api/src/container/ws.rs` - WebSocket ticket auth (mirrors `console::ConsoleManager`)
- `horcrux-api/src/container/ws_handlers.rs` - logs/exec/events WebSocket bridges
- `horcrux-api/src/db/migrations.rs` (migration 020) + `horcrux-api/src/db/mod.rs` (`stacks` module) - stack persistence
- `horcrux-api/horcrux-ui/src/pages/container_images.rs`,
  `container_volumes.rs`, `container_networks.rs`, `container_stacks.rs`,
  `container_events.rs`, `container_console.rs` - UI pages
- REST routes: `/api/images*`, `/api/volumes*`, `/api/container-networks*`,
  `/api/stacks*`, `/api/containers/:id/{logs,exec}-ticket`,
  `/api/containers/events-ticket`
- WebSocket routes: `/api/containers/ws/{logs,exec,events}/:ticket_id`
  (ticket-authenticated, same pattern as `/api/console/ws/*`)
