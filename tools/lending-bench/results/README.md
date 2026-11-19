Results of 2026-11-19: one 8 vCPU cloud VM (Xeon 6975P-C, kernel 7.0.0-1014-aws), RAM-backed NVMe/TCP target,
link modelled with tc on the loopback. `lending-bench-table.txt` and `qos-table.txt` are the first runs used in
docs/design/gaming-vm.md section 13; the `*-repeat*` files are a second run of the parameterised scripts
(10 GbE for the main bench, 1 GbE and 10 GbE for the QoS split). Tail latencies (p99.9 and up) vary by about 2x between runs.
`lending-bench-summary.json` is the machine-readable form of the first run.
