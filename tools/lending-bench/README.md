# lending-bench

Measures what running a game VM on a disk that lives on another machine costs (design: docs/design/gaming-vm.md,
section 13). One host plays both roles: the kernel NVMe/TCP target (`nvmet-tcp`, RAM-backed) and initiator (`nvme-tcp`)
talk over the loopback, and the link is modelled with `tc` (netem rate + delay, or htb for the QoS split) for the NVMe/TCP
port only. A real two-machine run needs no script change except `traddr` (a follow-up: step 7f in the design).

Files: `Dockerfile` (fio, nvme-cli, iproute2), `lending-bench.sh` (load / 4 KiB QD1 / stutter / small-read workloads
per link profile), `lending-bench-qos.sh` (stutter with and without a prioritised second connection),
`analyze.py` (tables), `results/`.

    docker build -t hx-lending-bench:1 .
    sudo -v; PROFILES="10GbE:10000:100 1GbE:1000:200" ./lending-bench.sh     # OUT=/var/tmp/hx-lending-bench/out
    python3 analyze.py /var/tmp/hx-lending-bench/out
    ./lending-bench-qos.sh && python3 analyze.py --qos /var/tmp/hx-lending-bench/out-qos

Needs root (sudo), docker, kernel modules brd nvmet nvmet_tcp nvme_tcp. It refuses to start when any of those
modules is already loaded, and undoes its qdiscs, configfs entries and modules on exit. Do not run it on a host
that serves NVMe-oF. Settings (environment): `OUT`, `IMG`, `RDKB` (target size in KiB), `PROFILES` (`name:rate_mbit:rtt_us`).
