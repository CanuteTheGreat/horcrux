#!/usr/bin/env python3
"""Summarise lending-bench fio runs.
  python3 analyze.py OUTDIR              table of the lending-bench.sh run, plus summary.json
  python3 analyze.py --qos OUTDIR        table of the lending-bench-qos.sh run
"""
import json, sys, glob, os

ORDER = ["local", "nvmetcp-loopback", "25GbE", "10GbE", "2.5GbE", "1GbE",
         "10GbE-rtt0.5ms", "10GbE-rtt2ms", "1GbE-rtt2ms"]

def load(d, n, w):
    p = os.path.join(d, f"{n}-{w}.json")
    if not os.path.exists(p):
        return None
    txt = open(p).read()
    return json.loads(txt[txt.index("{"):])

def pct(job, key, op="read"):
    d = job[op]
    lat = d.get("lat_ns") or d.get("clat_ns")
    return lat["percentile"].get(key, 0) / 1000.0, lat["max"] / 1000.0

if sys.argv[1:2] == ["--qos"]:
    for p in sorted(glob.glob(os.path.join(sys.argv[2], "*.json"))):
        j = json.loads(open(p).read()[open(p).read().index("{"):])
        fg = [x for x in j["jobs"] if x["jobname"] == "fg"][0]
        bg = [x for x in j["jobs"] if x["jobname"] == "bg"][0]
        pc = fg["read"]["lat_ns"]["percentile"]
        print("%-14s fg p50 %7.0f p99 %8.0f p99.9 %8.0f p99.99 %8.0f max %8.0f us | bulk %.2f GiB/s" % (
            os.path.basename(p)[:-5], pc["50.000000"] / 1e3, pc["99.000000"] / 1e3, pc["99.900000"] / 1e3,
            pc["99.990000"] / 1e3, fg["read"]["lat_ns"]["max"] / 1e3, bg["read"]["bw_bytes"] / 2**30))
    sys.exit(0)

rows = {}
for d in sys.argv[1:]:
    for n in ORDER:
        A, B, C, D = (load(d, n, w) for w in "ABCD")
        if not A:
            continue
        r = {}
        a = A["jobs"][0]["read"]
        r["load_s"] = A["jobs"][0]["job_runtime"] / 1000.0
        r["load_GiBs"] = a["bw_bytes"] / 2**30
        if B:
            p50, _ = pct(B["jobs"][0], "50.000000")
            p99, _ = pct(B["jobs"][0], "99.000000")
            p999, _ = pct(B["jobs"][0], "99.900000")
            p9999, mx = pct(B["jobs"][0], "99.990000")
            r.update(qd1_iops=B["jobs"][0]["read"]["iops"], qd1_p50=p50, qd1_p99=p99,
                     qd1_p999=p999, qd1_p9999=p9999, qd1_max=mx)
        if C:
            fg = [j for j in C["jobs"] if j["jobname"] == "fg"][0]
            bg = [j for j in C["jobs"] if j["jobname"] == "bg"][0]
            p50, _ = pct(fg, "50.000000")
            p99, _ = pct(fg, "99.000000")
            p999, _ = pct(fg, "99.900000")
            p9999, mx = pct(fg, "99.990000")
            r.update(c_p50=p50, c_p99=p99, c_p999=p999, c_p9999=p9999, c_max=mx,
                     c_bg_GiBs=bg["read"]["bw_bytes"] / 2**30)
        if D:
            r["small_kiops"] = D["jobs"][0]["read"]["iops"] / 1000.0
        rows[n] = r

def f(v, nd=1):
    return "-" if v is None else f"{v:.{nd}f}"

print("profile | load 6GiB s | GiB/s | 4k QD1 p50 us | p99 | p99.9 | max us | stutter fg p50 | p99 | p99.9 | p99.99 | max us | bg GiB/s | 4k QD32x4 kIOPS")
for n in ORDER:
    r = rows.get(n)
    if not r:
        continue
    print(" | ".join([n, f(r["load_s"]), f(r["load_GiBs"], 2), f(r.get("qd1_p50")), f(r.get("qd1_p99")),
                      f(r.get("qd1_p999")), f(r.get("qd1_max"), 0), f(r.get("c_p50")), f(r.get("c_p99")),
                      f(r.get("c_p999")), f(r.get("c_p9999")), f(r.get("c_max"), 0), f(r.get("c_bg_GiBs"), 2),
                      f(r.get("small_kiops"), 0)]))
json.dump(rows, open("summary.json", "w"), indent=1)
