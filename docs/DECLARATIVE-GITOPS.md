# Declarative configuration: stored desired state, drift, GitOps, history

Horcrux can describe a node in a spec file (storage pools, SDN, VMs; more kinds are being added), compare it
with the live node and apply only the difference. This page is the operator guide for the parts built on
top of `plan`/`apply`: the stored **desired spec**, **drift** detection, **GitOps** (follow a git repository),
and the **apply history** with **rollback**. The spec format itself and the engine's rules are in
`docs/design/declarative-config.md`.

Everything here is also in the web UI (Infrastructure > Declarative Config & GitOps, `/declarative`) and
the CLI. All of it works the same on OpenRC and systemd nodes: nothing in the engine talks to an init system.

## Quick start

```sh
horcrux export -f node.yaml                 # start from what the node has now, edit it
horcrux desired set -f node.yaml            # store it as the desired state
horcrux drift                               # exit 0 in sync, 2 when the node differs
horcrux desired apply --dry-run             # what applying would change
horcrux desired apply                       # make it so (recorded in the history)
horcrux history list
horcrux apply --rollback --dry-run          # what undoing the last apply would do
```

## Desired state and drift

`PUT /api/declarative/desired` stores the spec the node should follow (YAML or JSON text). The server parses
and validates it first and refuses a bad one (HTTP 422), so a stored spec always parses.

A background check plans the stored spec against the live node every `drift_interval_secs` (default 300,
5 to 86400; `PUT /api/declarative/settings`) and keeps the result for `GET /api/declarative/drift`
(`?refresh=true` re-checks now; `POST /api/declarative/drift/check` too). Each difference is one row:

| field | meaning |
|---|---|
| `kind`, `name` | the resource (`vm`, `storage_pool`, `zone`, `vnet`, `subnet`, ...) |
| `field` | the differing field, or `(resource)` for a missing/extra resource and `power` for run state |
| `expected` / `actual` | what the spec says / what the node has |
| `action` | what apply would do: `create`, `update`, `delete`, `start`, `stop` or `blocked` |
| `blocked`, `note` | the engine can't change it in place, and why |

What the spec leaves out is never drift. A section that is absent from the spec is unmanaged; with `prune` a
managed section's unlisted resources show as `delete` rows (a running VM is `blocked`: stop it first).

`GET /api/declarative/drift/diff` returns the spec and the matching slice of the live state as normalised
YAML, which is what the UI's "text diff" shows.

**Dashboard and alerts.** The dashboard has a "Config Drift" card (links to the Plan & drift tab; red only
while the count is above zero). Each check also feeds the alert system: metric `config_drift`, a rule named
"Configuration drift" (warning, fires while the count is above zero, created automatically; switch it off in
the Alert Center, or with `drift_alert: false` in the settings). Notification channels work as for any rule.

### In-place updates

`apply` changes existing resources without recreating them where the managers can:

| resource | changed in place | blocked (reported, never silently skipped) |
|---|---|---|
| VM | memory, CPUs, NICs (model, bridge, VLAN), CD-ROM; disks: grow, change bus, add | hypervisor, architecture, remove or shrink a disk, move a disk to another pool, CPU/memory of LXD/Incus VMs |
| storage pool | `enabled` | type, path |
| zone | all fields (unless the type changes under VNets) | |
| VNet | alias, tag, VLAN awareness | zone |
| subnet | gateway, SNAT | (the CIDR is part of its id) |

A running VM can take a CD-ROM change and a disk grow. Anything else cold (CPUs, memory, NICs, new disks,
disk bus) is planned as `blocked` with "stop it first", unless the spec says `power: stopped`: then the plan
is *stop, update* and the engine does it. Removing a disk detaches data, so it stays a manual step.

## GitOps

```yaml
# horcrux gitops config -f gitops.yaml      (or PUT /api/declarative/gitops/config, or the GitOps tab)
url: https://git.example.com/ops/fleet.git  # https://, ssh://, git@host:path, git://, file:// or a local path
branch: main
path: nodes/{node}.yaml                     # file in the repo; {node} = this node's name
interval_secs: 300                          # 10..86400
mode: approve                               # approve (default) | auto
verify_signatures: true
signing:
  format: ssh                               # ssh | gpg
  allowed_signers: |                        # inline, or allowed_signers_file: /abs/path
    ci@example.com ssh-ed25519 AAAAC3...
credentials: {username: bot, secret: "file:/etc/horcrux/git-token"}   # https only
ssh_key: "file:/etc/horcrux/git-key"        # for ssh URLs
webhook_secret: "env:GIT_WEBHOOK_SECRET"
node_name: pve-1                            # default: the host name
```

How a sync works (on the interval, on `POST /api/declarative/gitops/sync`, or when a webhook arrives):

1. Fetch `branch` with the `git` binary into a bare repository under the data directory
   (`<data_dir>/declarative/gitops`). No working tree, no hooks, no global git config; `ext::` and other
   transports are disabled.
2. With `verify_signatures`, verify the **HEAD** commit against the allowed signers. An unsigned commit, a bad
   signature or a signer that isn't listed is **refused**: nothing is parsed or applied, the status says why
   (`signature: bad`, `last_error: signature verification failed ...`), the API answers 422.
3. Read `path` at that commit, parse it (an invalid spec is refused with the parse error), plan it against the live node.
4. `auto`: apply it, and make it the desired state. Drift from the repository is corrected on every sync too,
   not only when the commit changes. `approve`: store the plan as a **pending approval** keyed by a hash of
   `commit + spec`, change nothing.

**Approval.** `POST /api/declarative/gitops/approve/<hash>` (UI: Approve and apply) fetches again and applies
only if the repository head is still the commit the plan was made for (and still correctly signed); otherwise
it answers 409, replaces the stale plan with one for the new head and tells you the new hash. `reject/<hash>`
drops the plan and remembers the commit so it isn't queued again; a new commit is. In approve mode drift is
shown the same way: the repository's plan against the drifted node waits for approval, nothing is corrected silently.
An in-sync poll writes no history entry.

**Status.** `GET /api/declarative/gitops/status`: last commit (sha, subject, author), signature state and
signer, last sync time, last result, last error, pending approvals. A node without `git` answers 424
(`git-missing`) and the UI shows what to install.

**Webhook.** `POST /api/declarative/gitops/webhook` is public but authenticated by an HMAC-SHA256 signature of
the raw body with `webhook_secret` (`X-Hub-Signature-256: sha256=...`, `X-Forgejo-Signature` and
`X-Gitea-Signature` are accepted). A valid push event for the configured branch wakes the sync loop; pushes to
other branches are ignored. The webhook never applies anything itself: the same fetch, signature and mode
rules apply. Without a configured secret the endpoint answers 404.

**Secrets are references, never values.** `credentials.secret`, `ssh_key` and `webhook_secret` accept
`file:/absolute/path`, `env:NAME` or `vault:path#key` (the node's Vault integration) and nothing else:
a plaintext value is rejected when the config is saved. Credentials are handed to git through `GIT_ASKPASS`
environment variables of the one child process; an SSH key is written 0600 for the duration of a fetch.
Credentials in the URL (`https://user:pw@...`) are refused. HTTPS credentials are never sent over plain http.

**Several nodes, one repository.** Put `{node}` in `path` (for example `nodes/{node}.yaml`): each node reads
its own file, named after `node_name` (default: the host name, sanitised to letters, digits, `.`, `_`, `-`).
This is the supported mechanism; there is no `nodes:` section in the spec. A file every node shares needs no
placeholder.

## History and rollback

Every real apply is recorded: id, time, actor, source (`api`, `cli`, `ui`, `gitops` with the commit,
`rollback`), spec hash and text, prune flag, the plan, and the per-change results. Dry runs and in-sync no-ops
are not. `GET /api/declarative/history` lists (newest first), `/history/<id>` shows one, and the cap
(`history_limit`, default 200, 10 to 5000) drops the oldest.

`POST /api/declarative/history/<id>/rollback` (`horcrux history rollback <id>`, `horcrux apply --rollback`
for the latest apply) undoes apply `<id>`:

* it takes the spec applied **before** `<id>` (or the one of the entry named by `to`) and plans it against the
  node as it is **now**, so field changes made by `<id>` are reverted and drift since then is handled too;
* resources `<id>` **created** that the older spec doesn't list are deleted (a running VM is never deleted:
  it is reported blocked); everything else the older spec doesn't mention is left alone, so a rollback can't
  remove something you made by hand;
* `dry_run: true` previews exactly that plan; the real run is recorded as a new entry (`source: rollback`,
  `rollback_of: <id>`); `set_desired: true` also stores the restored spec as the desired state.

Limits worth knowing: a rollback restores configuration, never data (a deleted VM's disk, an overwritten
volume); sections a kind doesn't expose as lists are restored but not pruned; with GitOps in `auto` mode the
next sync re-applies the repository head, so roll back by reverting the commit in git.

## API and CLI reference

| API | CLI |
|---|---|
| `GET/PUT/DELETE /api/declarative/desired`, `POST .../desired/validate`, `POST .../desired/apply` | `horcrux desired get/set/validate/apply/clear` |
| `GET /api/declarative/drift`, `POST .../drift/check`, `GET .../drift/diff` | `horcrux drift [--refresh]` |
| `GET/PUT /api/declarative/settings` | |
| `GET/PUT/DELETE /api/declarative/gitops/config`, `GET .../gitops/status`, `POST .../gitops/sync`, `.../approve/<hash>`, `.../reject/<hash>` | `horcrux gitops config/status/sync/approve/reject/disable` |
| `POST /api/declarative/gitops/webhook` | |
| `GET /api/declarative/history`, `GET .../history/<id>`, `POST .../history/<id>/rollback` | `horcrux history list/show/rollback`, `horcrux apply --rollback` |
| `POST /api/declarative/plan`, `POST .../apply`, `GET .../export` | `horcrux plan/apply/export` |

Permissions: reading (GET, plan, validate, drift check) needs `Sys.Audit` on `/`; changing (PUT, DELETE,
apply, sync, approve, reject, rollback) needs `Sys.Modify` on `/`; the webhook needs the HMAC signature only.

## Runtime requirements

`dev-vcs/git` (GitOps fetch), `net-misc/openssh` (SSH signatures use `ssh-keygen`, SSH remotes use `ssh`),
`app-crypt/gnupg` (OpenPGP signatures). They are dependencies of the `app-emulation/horcrux` and
`horcrux-bin` ebuilds.

## Tests

* Unit: `declarative::tests`, `declarative::gitops::tests`, `declarative::drift::tests` (plans, in-place updates,
  drift rows, config validation, the spec-vs-live restriction).
* Live server: `horcrux-api/tests/it/declarative_e2e.rs` (stored spec, drift and the alert, history, retention,
  rollback; GitOps against a real git repository with SSH-signed commits: auto, approve, reject, stale approval,
  `{node}` path, signature refusal, webhook-triggered sync).
* UI: `horcrux-ui` `tests/api_contract.rs` (the UI's types read the real responses) and `ci/ux-qa/ux_crawl.py`.
* Real guests: `tools/vmtest/scenarios/declarative_gitops.py`, two installed nodes (n1 auto, n2 approve) following
  one repository: converge, drift, approval, unsigned commit refused, scheduled sync. Run for the OpenRC and the
  systemd ISO in `.forgejo/workflows/vmtest.yml`.
