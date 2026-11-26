# Node maintenance

Any member of a Horcrux cluster can be put into maintenance from any other
member: the web UI (Cluster > Nodes, or HA > Maintenance with its node
picker), the API, the CLI or the declarative config. The request goes to the
node that serves it and is forwarded to the target over the cluster's
authenticated peer channel.

## Entering

Entering maintenance drains the node:

- every running VM is live-migrated to another member; a running container
  moves over the container transport (Incus/LXD move; LXC, Podman and Docker
  checkpoint, copy, restore where the runtime can, else stop, copy, start);
- every **stopped HA-managed guest** is moved offline, so its HA home is no
  longer the node in maintenance; it stays stopped on its new member (an
  offline move keeps the guest's requested HA state); stopped guests outside
  HA stay;
- targets are picked by free memory, CPU architecture, required passthrough
  devices and the guest's placement rules (pin, affinity, anti-affinity);
- while the node is in maintenance, HA and the rebalancer never place a guest
  on it, and the node going down is reported as expected, not as a failure:
  no HA recovery is started for it (a node that stops answering while it
  shuts down keeps its last known maintenance state);
- shutting the host down while it is in maintenance does not end the
  maintenance: after the boot it is still in maintenance until an
  administrator leaves it (only a maintenance that the shutdown drain itself
  started ends at boot).

Before anything moves, the plan (`GET .../maintenance/plan`) lists every
guest, its target and, for guests that cannot move, the reason (pinned,
no member with enough memory, architecture, a passthrough device). Entering
refuses with that list (409) unless `force` is set; with `force` the guests
that cannot move stay where they are (and are saved or shut down with the
host as the guest-power settings say).

Each moved guest's origin is recorded with the drain run
(`origins` in the maintenance state).

## Leaving

Leaving maintenance applies a return policy:

| policy | what happens |
|---|---|
| `origin` (default) | every guest the drain moved comes back to this node (running ones live-migrated), unless its placement rules now forbid it or the node lacks room; those stay and the exit run lists why |
| `rebalance` | the rebalancer evens out the cluster (moves only if the imbalance exceeds its threshold) |
| `none` | guests stay where they are |

The policy comes from the request (`return`), else the node's own setting,
else the cluster default `cluster.maintenance.return_policy`.

## API

| route | method | what |
|---|---|---|
| `/api/nodes/:node/maintenance` | GET | state of that member: `enabled`, `since`, `reason`, `drain` run, `exit` run, `origins`, `return_policy`, `effective_return_policy` |
| `/api/nodes/:node/maintenance` | POST | `{"enabled": true\|false, "drain": true, "force": false, "return": "origin"\|"rebalance"\|"none"}` |
| `/api/nodes/:node/maintenance/plan` | GET | `?action=enter` (default) or `?action=exit&return=<policy>`: the moves and the guests that cannot move, nothing changes |
| `/api/nodes/maintenance` (+ `/plan`) | GET/POST | the same for the node serving the request |
| `/api/cluster/maintenance` | GET | every member's state (badges) and the cluster settings |
| `/api/cluster/maintenance` | PUT | `{"return_policy": "origin"}`: the cluster default |

Privileges: reading a node's maintenance needs `Sys.Audit` on
`/nodes/<node>`, changing it `Sys.Modify` on `/nodes/<node>`; the cluster
default needs `Sys.Modify` on `/cluster`.

## CLI

```
horcrux node maintenance <node> status
horcrux node maintenance <node> plan [--return origin|rebalance|none]
horcrux node maintenance <node> on [--force] [--no-drain]
horcrux node maintenance <node> off [--return origin|rebalance|none]
```

## Declarative config

```yaml
cluster:
  maintenance:
    return_policy: origin        # default for every member
nodes:
  node-b:
    maintenance:
      enabled: true
      return_policy: none        # optional, overrides the cluster default for node-b
```

`horcrux export` writes both sections, `horcrux plan` shows the change and
`horcrux apply` enters or leaves maintenance through the same code path as the API
(a member that cannot be reached is reported, not skipped silently).

## Tests

- planner unit tests: `crates/horcrux-cluster/src/cluster/drain.rs`
  (stopped HA guests evacuated, origins recorded and merged, return plans);
- routes and RBAC: `horcrux-api/tests/it/node_maintenance_tests.rs`, plus
  the `rbac_matrix` / `every_route` scans;
- containers in docker siblings: `horcrux-api/tests/drain_e2e.py`,
  `drain_ct_vm_e2e.py`;
- three installed nodes from the ISO (OpenRC and systemd):
  `tools/vmtest/scenarios/cluster_maintenance.py`.
