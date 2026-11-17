# Declarative cluster configuration

Goal: NixOS-style configuration. A cluster's desired state lives in version
control as a spec file; Horcrux diffs it against the live state and applies
only the difference, idempotently. Phase 1 covered **storage pools, SDN
networks and VMs** with `export`, `plan` and `apply`. Phases 2 (stored desired
state, drift, in-place updates), 3 (GitOps) and 5 (apply history and rollback)
are implemented too and documented for operators in `docs/DECLARATIVE-GITOPS.md`.
The **resource kinds** of phase 4 (containers, container volumes,
users/groups/roles/ACL, firewall, backup jobs, HA groups) are in as well, each
with `export`, `plan` and `apply` (see "Resource kinds" below); the cluster
fan-out (`nodes`) is still open.

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
- Keys that are not supported yet (`nodes`: cluster fan-out) are *rejected*
  with "not supported yet" rather than ignored, so a spec never half-applies
  without saying so. The same goes for a typo inside a kind (`memroy:`): the
  kinds below reject unknown fields.

## Resource kinds (phase 4, per node)

Every kind follows the same rules: a pure `plan` (unit-tested), `apply` through
the managers the REST handlers use, `export` as a fixed point
(`export` -> `plan --prune` is empty), prune only inside a section the spec
lists, and **secrets only as references**. Code: one `Kind` implementation per
section in `horcrux-api/src/declarative/kinds/` (the engine's extension point,
registered in `kinds/mod.rs`); `gather` returns the live section in spec shape,
which is also what `export` writes. What a plan needs that is not spec-shaped
(who is applying and from where, the API port, runtime probes, password hashes,
built-in roles, HA usage) is in `Live::kinds` (`kinds/ctx.rs`), gathered once.
The caller's identity reaches the kinds through a tower layer on the
declarative router, so every endpoint under it (plan, apply, desired/apply,
rollback...) is covered without its handler knowing.

| section | matched by | apply uses |
|---|---|---|
| `containers` | name | `ContainerManager` (create/start/stop/delete), probe for runtimes |
| `volumes` | runtime + name | Podman/Docker volume managers |
| `roles`, `users`, `groups`, `acl` | name / username / name / (path, subject, role) | auth store; `create_user`/`delete_user` handlers |
| `firewall` | ids per scope; alias/ipset/group names | `FirewallManager` |
| `backup_jobs` | id | `BackupManager` (USE=backup) |
| `ha_groups` | name | `HaManager` (USE=cluster), then the peer broadcast |

A section the node cannot manage (feature compiled out) is a plan error when
the spec names it, and export leaves it out.

### containers
```yaml
containers:
  - name: web
    image: docker.io/library/nginx:1.27   # image, or the LXC template / LXD-Incus image
    runtime: podman      # podman | docker | lxc | lxd | incus | system
    memory: 256          # MB; cpus: 1   (absent = not limited, not compared)
    cpus: 1
    ports: ["8080:80"]
    env: {TZ: UTC}       # non-secret only
    mounts: ["data:/var/lib/data", "/srv/www:/usr/share/nginx/html:ro"]
    command: [nginx, -g, "daemon off;"]
    network: backend
    restart: unless-stopped   # no | always | unless-stopped | on-failure[:N]
    power: running
    replace: false       # true: a differing container is deleted and re-created
```
- **Runtime default:** no `runtime` means Podman, falling back to Docker when
  only Docker works. `system` means Incus, else LXD (Incus is preferred over
  LXD, Podman over Docker). Docker, LXC and LXD stay fully supported when named.
- **Missing runtime:** the create is `blocked` with the node's own
  `node lacks <reason>` text (the same reason codes as the 424 answers), and
  the rest of the plan goes on.
- **No in-place edit:** a differing container is `blocked` unless the spec
  says `replace: true` (stop, delete, create again; named volumes are kept).
  Power is reconciled either way.
- **What can be read back:** Horcrux tracks a container's limits and options
  in memory, so after an API restart a container reads as "limits 0, no
  options". The plan treats those as unknown, never as drift; image, runtime
  and power are always compared (images after normalising `docker.io/library/`
  and `:latest`).
- **Secrets:** an env key that looks secret (`*PASSWORD*`, `*TOKEN*`,
  `*SECRET*`, `API_KEY`...) is refused in a spec and never exported; mount a
  secret file instead. Prune never deletes a running container.
- `lxc:` settings (LXC rootfs/network/idmap options) are not part of the spec yet.

### volumes
`volumes: [{name: data, runtime: podman, driver: local}]`. Volumes hold data:
prune never deletes one (it is `blocked` with the reason), and a different
driver is `blocked`. Podman is the default runtime.

Storage beyond pools and volumes has no manager that fits: NAS shares need
per-protocol daemons and their own feature flags, so they are not part of the
spec yet.

### users, groups, roles, ACL
```yaml
roles:   [{name: Operator, privileges: [VM.Audit, VM.PowerMgmt]}]   # custom roles only
users:
  - username: alice                    # name@realm for an external realm
    email: alice@example.org
    password_hash: "$argon2id$..."     # a hash, or
    password_ref: {file: /etc/horcrux/secrets/alice}   # or {env: NAME} / {vault: path, key: k}
groups:  [{name: ops, members: [alice]}]   # members absent = membership not managed
acl:     [{path: /vms, subject_type: group, subject: ops, role: Operator}]
```
- **Passwords never appear in a spec:** a plaintext `password:` is an unknown
  field; `password_hash` must be an argon2 hash; `password_ref` is read on the
  server at plan/apply time and compared against the live hash (a rotated
  secret shows up as an `update`, which also ends the user's sessions).
- **Export omits hashes.** `GET /api/declarative/export?include_hashes=true`
  (`horcrux export --include-hashes`, written 0600) adds them and needs
  `Permissions.Modify` on `/`. Without that privilege the export leaves
  users, groups, roles and ACL out altogether.
- **Safety, in the plan and again in the managers:** applying these sections
  needs `Permissions.Modify` on `/` (more than the `Sys.Modify` apply itself
  needs: otherwise a Sys.Modify holder could promote itself); the calling user
  is never deleted, disabled or given a new password through apply; a plan
  that would leave no enabled administrator (counting grants through groups)
  is refused as a whole; references (ACL -> role/user/group, members -> user)
  must resolve in the resulting state; built-in roles are never redefined,
  pruned or exported.
- Deleting a user, group or role takes its grants with it (the schema does).
- Work with no HTTP caller (the drift timer, a scheduled sync) can read these
  sections but cannot change them, and its plans skip the calling-user checks;
  to allow it, run the apply inside `kinds::ctx::CALLER.scope(..)` with a
  service identity. A `password_ref` in a `{vault: ...}` form is only checked
  when the user is created or another field changes (the plan can read files
  and environment variables, not the vault).

### firewall
```yaml
firewall:
  options: {policy_in: accept, policy_out: accept}
  aliases:  [{name: office, cidr: 192.0.2.0/24}]
  ipsets:   [{name: admins, entries: [192.0.2.10]}]
  security_groups: [{name: web, rules: [{id: https, action: ACCEPT, protocol: tcp, dport: "443"}]}]
  rules:
    - {scope: datacenter, id: ssh, action: ACCEPT, protocol: tcp, source: office, dport: "22"}
    - {scope: vm:web1, id: https, action: ACCEPT, protocol: tcp, dport: "443"}
  allow_lockout: false
```
Scopes: `datacenter`, `node:<name>`, `vm:<id>`, `container:<id>`.
- **Order is part of the state.** A scope that appears in `rules` is an ordered
  list. The manager has no edit or reorder call, so a changed or displaced
  rule and everything after it is removed and re-added (shown as `update`
  with "re-added to keep the rule order"). Unlisted live rules of a listed
  scope stay at the end (no prune) or go (prune); a scope that is no longer
  listed is emptied by prune.
- Host scopes get a final `fw_reload` step (after every delete) that loads
  them into nftables; guest scopes are synced by the manager.
- **Security groups are create-only:** the manager has no update or delete, so
  a difference (or a prune) is `blocked` with that reason, never ignored.
- **Anti-lockout.** The manager keeps `22, 8006, 8026` open when the inbound
  policy is `drop`. With policy `accept`, a plan that newly adds a rule which
  drops or rejects the API port - for the caller's address or for everyone,
  by port, range or macro, through aliases and IP sets, ahead of any matching
  accept - is a plan error and nothing is applied. `allow_lockout: true`
  overrides it. A rule already in place is not this plan's problem. Rules
  that name an alias, `+ipset` or security group that does not exist are plan
  errors too.

### backup_jobs
`backup_jobs: [{id: nightly, schedule: "0 2 * * *", targets: [web], storage: local, mode: snapshot, compression: zstd, retention: {keep_last: 3}, notify: failure}]`.
Run history is server state, never drift. Deleting a job keeps its backups.

### ha_groups
`ha_groups: [{name: g1, nodes: [n1, n2], restricted: false, no_failback: false}]`.
A group HA resources still use is never deleted (`blocked`). HA *resources*
(which guests are protected) are not in the spec yet.

### Apply order
`storage_pool, zone, vnet, subnet, vm, volume, container, role, user, group,
acl, fw_alias, fw_ipset, fw_security_group, fw_rule, fw_options, backup_job,
ha_group, fw_reload`; deletes run in the reverse order, `fw_reload` last.

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
- In-place updates (phase 2): zones, VNet alias/tag/VLAN awareness, subnet
  gateway/SNAT, pool `enabled`, VM memory/CPUs/NICs/CD-ROM and disk grow/bus/add
  (cold changes of a running VM are planned as stop + update when the spec says
  `power: stopped`, otherwise `blocked`). **Everything else that differs is
  `blocked`** with a reason: pool type/path, VNet zone, VM hypervisor/architecture,
  disk shrink/removal/pool move. Power state is reconciled for existing VMs.
- Every `Change` carries per-field `{field, expected, actual}` (`fields`), which
  is what the drift view shows.

### Apply
- One apply at a time per node (`409` if another is running).
- Best effort: a failed change is reported and the rest continue (later
  changes that depended on it fail with the manager's message). The response
  is `{ok, plan, results[{.., status: applied|failed|skipped, message}]}`;
  `ok` is false if anything failed or was blocked.
- Re-running is safe: applied changes no longer show up in the next plan.
- Every real apply is recorded in the history and can be rolled back (phase 5).

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
(it changes pools, networks, VMs, containers, firewall... node-wide). The
access-control sections add `Permissions.Modify` on `/` (above).

Scope today: **per node** (like the SDN config it drives). A cluster spec
that fans out to nodes is phase 4.

## Phases

1. **(done)** Spec v1, export/plan/apply for pools, SDN, VMs; CLI; e2e.
2. **(done) Drift + stored desired state.** `PUT /api/declarative/desired` keeps the
   spec the node should follow; a periodic drift check (plan against live,
   `drift_interval_secs`, default 5 min) feeds `GET /api/declarative/drift`
   (per field expected/actual), a dashboard badge and the "Configuration drift"
   alert rule (metric `config_drift`). In-place updates: VM memory/CPUs/NICs/
   disks, pool enable/disable, VNet tag/alias, subnet gateway/SNAT.
3. **(done) GitOps sync.** `declarative.git: {url, branch, path, interval, mode:
   auto|approve, verify_signatures, signing}`: the `git` binary fetches into a
   state dir, the HEAD signature is verified (SSH or OpenPGP, refuse unsigned or
   unknown signers), the spec is parsed and planned. `auto` applies,
   `approve` stores the plan keyed by `hash(commit, spec)` and waits for
   `POST .../gitops/approve/<hash>` (applies only if the head is still that
   commit) or `reject`. HMAC-signed push webhook; status with last commit,
   last sync, last error, pending. Per-node files through `{node}` in the path.
   Credentials are references (`file:`, `env:`, `vault:`), never values.
4. **More kinds + cluster fan-out.** **(kinds done)** containers, volumes,
   users/groups/roles/ACLs (secrets as references, never plaintext in git),
   firewall, backup jobs, HA groups, each a `declarative::kinds::Kind` in its
   own file. Open: `nodes` and a cluster-level spec with per-node sections
   applied through the peer channel, HA resources, NAS shares, LXC option
   blocks.
5. **(history + rollback done; the rest open) Safety rails.** Done: every apply
   (API, CLI, UI, GitOps, rollback) recorded with actor, source, commit, spec,
   plan and results, retention cap, `POST /history/<id>/rollback` (plan the
   previous spec against the live node, dry-run preview, apply, recorded as a
   new entry; only what the rolled-back apply created is deleted),
   `horcrux apply --rollback`. Open: a config-store snapshot taken before each
   apply, change windows, per-kind prune allow-lists.

## Testing
- Unit (`declarative::tests`, `gitops::tests`, `drift::tests`): parsing/validation,
  ordering, ownership, prune safety, drift rows, in-place update planning, the
  export fixed point, GitOps config validation.
- Unit (`declarative::kinds::*::tests`, `declarative::tests::kinds_integration`):
  per kind plan, ordering, prune, ownership, safety rules (last admin, caller,
  lockout), validation, export fixed point and "export never leaks".
- E2E kinds (same file): real accounts that log in (and a rotated secret), a
  Sys.Modify-only caller refused, firewall rules in order and the refused
  lockout, backup jobs, HA groups, and containers/volumes on a real runtime when
  the host has one (otherwise the `node lacks` path).
- E2E (`horcrux-api/tests/it/declarative_e2e.rs`): a spawned server; real
  pool dir, SDN zone (+ VNet/subnet when bridges are creatable), VMs;
  apply, in-place updates, idempotence, export fixed point, prune, teardown,
  auth; stored desired spec, drift + alert, history, retention, rollback;
  GitOps against a real git repository (auto, approve, reject, stale approval,
  signature refusal with real SSH signatures, `{node}` paths, webhook).
- Real guests (`tools/vmtest/scenarios/declarative_gitops.py`): two installed nodes
  following one repository, OpenRC and systemd ISO.
