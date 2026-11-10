# Quorum votes per node

Every node carries one corosync vote by default. For a mixed cluster (for example 10 nodes with two weak 2006-era servers) an operator can change a node's weight:

* Web UI: Cluster Management > node row > "votes" field next to role and limits > Save (empty = 1, 0 = never counts toward quorum, max 10).
* API: `PUT /api/cluster/nodes/:name/caps` with `"quorum_votes": N` in the node's caps body (rejected above 10).
* `GET /api/cluster/quorum/plan` shows the resulting quorum threshold, tolerated vote loss, `auto_tie_breaker` for an even vote total, warnings and the corosync.conf that is written.

Changing the weight rewrites `corosync.conf` on the node that initialised the cluster and reloads corosync (`corosync-cfgtool -R`). A node added later takes its weight from its caps.
