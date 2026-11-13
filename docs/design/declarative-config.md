# Declarative cluster configuration

Goal: NixOS-style configuration. A cluster's desired state lives in version
control as a spec file; Horcrux diffs it against the live state and applies
only the difference, idempotently. Phase 1 (this document's implemented part)
covers **storage pools, SDN networks and VMs** with `export`, `plan` and
`apply`. GitOps sync, drift in the UI and the remaining resource kinds are
phases 2-4 below.

## Principles

1. **The spec is the input, the API is the engine.** `apply` calls the same
   managers and SDN handlers as the individual REST calls, so validation and
   side effects are identical. It never shells out to an init system, so it
   behaves the same on OpenRC and systemd nodes (the e2e test uses neither).
2. **Plan before apply, and plan is pure.** `plan` = `apply --dry-run`; it
   never changes anything. The diff logic (`horcrux-api/src/declarative/mod.rs`)
   has no I/O and is unit-tested exhaustively.
3. **Ownership is explicit.** Resources not in the spec are left alone unless
   `prune` is on, and prune only touches *managed sections* (below).
4. **Never silently drop a difference.** What phase 1 cannot change in place
   is reported as `blocked` and makes `apply` exit non-zero.
5. **Fixed point.** `export` followed by `plan --prune` of the result is empty.

## Spec format (version 1, YAML or JSON)

```yaml
version: 1            # required to match the server's SPEC_VERSION
prune: false          # default; `--prune` / request field `prune` overrides

storage_pools:        # key = name
  - {name: fast, type: directory, path: /srv/fast, enabled: true}

networks:
  zones:   [{id: lab, type: simple}]            # type: simple | vlan | vxlan
  vnets:   [{id: web, zone: lab}]               # becomes bridge `web`
  subnets: [{vnet: web, cidr: 10.20.0.0/24, gateway: 10.20.0.1, snat: true}]

vms:                  # key = name
  - name: app1
    id: app1          # optional, default = name
    hypervisor: qemu  # qemu | lxd | incus
    memory: 2048      # MB
    cpus: 2
    architecture: x86_64      # optional (default: host arch)
    disks:
      - {size_gb: 20, bus: virtio, storage: fast}   # storage = pool NAME
    hardware: {nets: [{bridge: web, model: virtio}]}
    display: {protocol: none}
    image: null       # LXD/Incus instance image
    power: running    # running | stopped | (absent = leave alone)
```

Notes:
- Cross references use names/ids from the spec (`storage:` is a pool *name*,
  resolved to the pool id at apply time; `vnet.zone`, `subnet.vnet`).
  A reference to something that already exists live is fine too.
- Subnet ids are derived (`<vnet>-<net>-<prefix>`); the spec does not set them.
- VMs match by **name**; storage pools by **name**; zones/vnets by **id**;
  subnets by derived id.
- Keys for later phases (`containers`, `users`, `acl`, `firewall`,
  `backup_jobs`, `ha_groups`, `nodes`) are *rejected* with "not supported
  yet" rather than ignored, so a spec never half-applies without saying so.

## Semantics

### Managed sections and prune
A top-level list that is **absent** is unmanaged: Horcrux never deletes
anything of that kind. A list that is **present** is managed: with
`prune: true` live resources of that kind missing from the spec are deleted
(`storage_pools: []` + prune = "no pools should exist"). Without prune,
unlisted resources are always kept. The `networks` sub-lists (zones, vnets,
subnets) are managed independently.

Prune safety: a running VM is never deleted (reported `blocked`: stop it
first). Dependents the manager refuses to delete (a pool holding disks, a
zone with VNets) fail with the manager's message.

### Plan
`plan(spec, live, prune) -> {changes[], errors[]}`:
- `changes[]`: `{kind, name, action, details[]}`; action is `create`,
  `update`, `delete`, `start`, `stop` or `blocked`.
- Order: creates/updates dependency-first (pools, zones, vnets, subnets,
  VMs), then deletes in reverse (VMs first, pools last).
- `errors[]`: the spec cannot be applied (dangling reference, invalid VNet
  for its zone). `apply` refuses to start while there are errors.
- Compared fields: pool type/path/enabled; zone all fields; VNet and subnet
  all fields; VM hypervisor, memory, cpus, architecture (if stated), disks
  (count, size, bus, pool) if stated, NICs (model, bridge, VLAN; MAC only if
  stated) and CD-ROM if stated. Fields the spec leaves out are never drift.
- Phase-1 update support: zones update in place; **everything else that
  differs is `blocked`** (VNet/subnet/pool edits and VM reconfiguration need
  new in-place APIs, phase 2). Power state is reconciled for existing VMs.

### Apply
- One apply at a time per node (`409` if another is running).
- Best effort: a failed change is reported and the rest continue (later
  changes that depended on it fail with the manager's message). The response
  is `{ok, plan, results[{.., status: applied|failed|skipped, message}]}`;
  `ok` is false if anything failed or was blocked.
- Re-running is safe: applied changes no longer show up in the next plan.
- No rollback in phase 1; plan first (or review the GitOps PR).

### Export
`GET /api/declarative/export` returns the live state as a spec with every
section present (so prune on it is meaningful). MACs are kept so re-creating
VMs from an export reproduces the NICs. Volatile/runtime fields (status,
disk paths, usage) are omitted except `power`.

## Interfaces

| | |
|---|---|
| `GET  /api/declarative/export` | `{spec: <yaml text>, parsed: {...}}` |
| `POST /api/declarative/plan`   | `{spec, prune?}` -> `{in_sync, plan}` |
| `POST /api/declarative/apply`  | `{spec, prune?, dry_run?}` -> report |
| `horcrux export [-f out.yaml]` | |
| `horcrux plan -f spec.yaml [--prune]` | exit 0 in sync, 2 pending |
| `horcrux apply -f spec.yaml [--prune] [--dry-run]` | exit non-zero if not all applied |

`spec` is YAML/JSON text or a JSON object. RBAC: new area `declarative`;
export and plan need `Sys.Audit` on `/`, apply needs `Sys.Modify` on `/`
(it changes pools, networks and VMs node-wide).

Scope today: **per node** (like the SDN config it drives). A cluster spec
that fans out to nodes is phase 4.

## Phases

1. **(done)** Spec v1, export/plan/apply for pools, SDN, VMs; CLI; e2e.
2. **Drift + stored desired state.** `PUT /api/declarative/desired` keeps the
   spec the node should follow; a periodic drift check (plan against live)
   feeds `GET /api/declarative/drift`, a dashboard badge and an alert rule.
   In-place updates: VM memory/cpus/NICs/disks (stopped VM or hot-plug),
   pool enable/disable, VNet tag/alias, subnet gateway/snat.
3. **GitOps sync.** `declarative.git: {url, branch, path, interval, mode:
   auto|approve}`: fetch (libgit2 or `git` binary), parse, plan; `auto`
   applies, `approve` stores the plan and waits for `POST .../approve/<hash>`
   (UI button / CLI). Webhook trigger for push events; sync status + last
   commit in the API. Signed-commit verification option.
4. **More kinds + cluster fan-out.** containers, users/groups/ACLs (secrets
   as references, never plaintext in git), firewall, backup jobs, HA groups,
   nodes; a cluster-level spec with per-node sections applied through the
   peer channel.
5. **Safety rails.** Pre-apply snapshot of the config store with
   `horcrux apply --rollback`, change windows, per-kind prune allow-lists.

## Testing
- Unit (`declarative::tests`): parsing/validation, ordering, ownership,
  prune safety, drift detection, the export fixed point.
- E2E (`horcrux-api/tests/it/declarative_e2e.rs`): a spawned server; real
  pool dir, SDN zone (+ VNet/subnet when bridges are creatable), VMs;
  apply, idempotence, export fixed point, drift, prune, teardown, auth.
