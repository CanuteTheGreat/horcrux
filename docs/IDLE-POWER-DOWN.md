# Idle power-down (lab nodes)

Opt-in per node: give a node the label `power-managed` (Cluster > Cluster Management > node caps).
Only labelled nodes are ever considered.

A labelled node is a power-down candidate when it is powered on, has no running guests, CPU is below
the idle threshold (default 5%), is not pinned by HA or PCI passthrough, hosts no Ceph OSDs, and
removing it keeps quorum and at least `min_online` (default 3) nodes online.

- UI: Cluster Management > "Idle power-down" panel lists every node with CPU, guests, votes, wake
  method and the reason it is or is not a candidate. Read-only.
- API: `GET /api/cluster/power/status` (live sample + plan), `POST /api/cluster/power/plan` (dry run
  on supplied nodes), `POST /api/cluster/power/wake` (Wake-on-LAN), `POST /api/cluster/power/bmc-on`
  (IPMI/iLO/DRAC/Redfish power-on through the node's fencing device).

- `POST /api/cluster/power/down` `{"node":"x"}` gracefully powers off one idle node through its BMC
  (ipmitool `power soft`, Redfish `GracefulShutdown`). It is refused with 409 unless the planner
  currently lists the node as a candidate, and never targets the node serving the request.

Nothing powers a node off automatically yet; the plan is advisory and shutdown is an explicit call.
