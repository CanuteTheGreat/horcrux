# Quorum votes per node

Every node carries one corosync vote by default. For a mixed cluster (for example 10 nodes with two weak 2006-era servers) an operator can change a node's weight:

* Web UI: Cluster Management > node row > "votes" field next to role and limits > Save (empty = 1, 0 = never counts toward quorum, max 10).
* API: `PUT /api/cluster/nodes/:name/caps` with `"quorum_votes": N` in the node's caps body (rejected above 10).
* `GET /api/cluster/quorum/plan` shows the resulting quorum threshold, tolerated vote loss, `auto_tie_breaker` for an even vote total, warnings and the corosync.conf that is written.

Changing the weight rewrites `corosync.conf` on the node that initialised the cluster and reloads corosync (`corosync-cfgtool -R`). A node added later takes its weight from its caps.

## External quorum device (qdevice)

For a two-node (or small even) cluster, set it with `PUT /api/cluster/quorum/qdevice` (`{"host":"qnetd.lab","algorithm":"ffsplit"}`; `null` clears; `GET` shows it; stored in `/etc/horcrux/qdevice.conf`, override path with `HORCRUX_QDEVICE_FILE`) or set `HORCRUX_QDEVICE_HOST=<qnetd host or IP>` (and optionally `HORCRUX_QDEVICE_ALGO=ffsplit|lms`, default `ffsplit`) in the service environment on the node that initialises the cluster. The generated `corosync.conf` then gets a `device { model: net ... tls: on }` block inside `quorum {}`: `ffsplit` adds one vote (decides a 50/50 split), `lms` adds nodes-1 votes. `auto_tie_breaker` is not written while a qdevice is configured (corosync cannot combine them). Run `corosync-qnetd` on a separate machine and `corosync-qdevice` on every node. `GET /api/cluster/quorum/plan` shows `qdevice_host`, `qdevice_votes` and the resulting totals. Only letters, digits, `.`, `-` and `:` are accepted in the host; anything else is ignored.
