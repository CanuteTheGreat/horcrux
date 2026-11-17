# HA failover: measured on real hardware

Goal G1: a guest is running again on a surviving node in under 90 s after a node failure.

The cluster HA suite (`scenarios/hasuite.py`, job `cluster-hasuite`) runs on three physical machines
with a shared NFS datastore, a checksummed in-guest workload and real hard crashes (sysrq-b), manager
kills and network partitions. Latest run: 2026-11-17, build 2ef3e908 (includes the standalone fence agent).

| scenario | detection s | failover s | guest outage s | data integrity | double-run s |
|---|---|---|---|---|---|
| hard crash (node reboots by itself) | 8.0 | 35.1 | 43 | OK | 0 |
| hard crash, node stays down | 11.0 | 33.1 | 40 | OK | 0 |
| forced power-off | 10.9 | 37.7 | 43 | OK | 0 |
| network partition, minority self-fences | 8.0 | 38.6 | 44 | OK | 0 |
| manager hung | 6.0 | 16.0 | 6 | OK | 0 |
| node replacement (removal + re-join) | - | 23.9 | - | OK | - |

Every timed row meets G1; the worst is 38.6 s. Not covered by the 90 s target:

- Quorum loss (two nodes crash at once): the guest runs again 126 s after the double crash, once quorum is back.
- Guests on node-local disks are not highly available; they wait for their node.
- A node with its manager stopped at boot is recovered through the standalone `horcrux-ha-fence` service
  (`GET /attest` on port 8026), so a manager-less node no longer holds its guests for minutes.

Wake-on-LAN from a forced power-off is hardware-dependent and is reported separately, not counted in failover.
