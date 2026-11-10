# Idle power-down (lab nodes)

Opt-in per node: give a node the label `power-managed` (Cluster > Cluster Management > node caps).
Only labelled nodes are ever considered.

A labelled node is a power-down candidate when it is powered on, has no running guests, CPU is below
the idle threshold (default 5%), is not pinned by HA or PCI passthrough, hosts no Ceph OSDs, and
removing it keeps quorum and at least `min_online` (default 3) nodes online.

- UI: Cluster Management > "Idle power-down" panel lists every node with CPU, guests, votes, wake
  method and the reason it is or is not a candidate. A candidate row has a "Power off" button (asks to confirm, then calls `POST /api/cluster/power/down`).
- API: `GET /api/cluster/power/status` (live sample + plan), `POST /api/cluster/power/plan` (dry run
  on supplied nodes), `POST /api/cluster/power/wake` (Wake-on-LAN), `POST /api/cluster/power/bmc-on`
  (IPMI/iLO/DRAC/Redfish power-on through the node's fencing device).

- `POST /api/cluster/power/down` `{"node":"x"}` gracefully powers off one idle node through its BMC
  (ipmitool `power soft`, Redfish `GracefulShutdown`). It is refused with 409 unless the planner
  currently lists the node as a candidate, and never targets the node serving the request.

## Automatic power-down (opt-in)

Off by default. Set `HORCRUX_IDLE_POWER=1` in the service environment on every node to let the
cluster shut idle `power-managed` nodes down by itself:

- Only the leader (lowest-named online node) acts, and at most one node per sample.
- A node must be a planner candidate (all rules above) for `HORCRUX_IDLE_POWER_SAMPLES` consecutive
  samples (default 3), taken `HORCRUX_IDLE_POWER_INTERVAL_SECS` apart (default 300, minimum 10), so
  a short CPU dip never powers a node off. Any busy sample restarts the count.
- The shutdown is the same guarded graceful BMC call as `POST /api/cluster/power/down`; a refusal
  is logged as a warning and retried after a fresh streak.

Waking is still explicit (`/power/wake`, `/power/bmc-on`); there is no automatic wake-on-demand yet.
