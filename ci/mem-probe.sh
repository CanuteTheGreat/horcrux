#!/usr/bin/env bash
# CI build-memory probe: records the job container's memory use so every
# Rust job reports what it really needs (target: <= 6 GB per job).
#
#   ci/mem-probe.sh start          start a 1 s sampler (background)
#   ci/mem-probe.sh mark <label>   start a new segment (call at the top of a step)
#   ci/mem-probe.sh report         print per-segment peaks + the cgroup's memory.peak
#
# Per segment it reports: wall time, peak cgroup memory (memory.current,
# includes page cache), peak anonymous memory (what really has to fit; page
# cache is reclaimable) and the largest single process (usually one rustc or
# the linker) with its RSS. Works with cgroup v2 (private cgroupns, the
# Docker default) and falls back to cgroup v1 counters.
set -u
dir=/tmp/mem-probe
cg=/sys/fs/cgroup

read_mem() {
    if [ -r "$cg/memory.current" ]; then
        cur=$(cat "$cg/memory.current")
        anon=$(awk '$1=="anon"{print $2}' "$cg/memory.stat")
    elif [ -r "$cg/memory/memory.usage_in_bytes" ]; then
        cur=$(cat "$cg/memory/memory.usage_in_bytes")
        anon=$(awk '$1=="total_rss"{print $2}' "$cg/memory/memory.stat")
    else
        cur=0
        anon=0
    fi
}

case "${1:-}" in
start)
    mkdir -p "$dir"
    : > "$dir/samples"
    echo "$(date +%s) setup" > "$dir/marks"
    if [ -f "$dir/pid" ] && kill -0 "$(cat "$dir/pid")" 2>/dev/null; then
        exit 0
    fi
    nohup setsid "$0" _loop > /dev/null 2>&1 &
    echo $! > "$dir/pid"
    ;;
_loop)
    while :; do
        read_mem
        top=$(ps -eo rss=,comm= --sort=-rss 2>/dev/null | head -1)
        echo "$(date +%s) ${cur:-0} ${anon:-0} ${top:-0 ?}" >> "$dir/samples"
        sleep 1
    done
    ;;
mark)
    mkdir -p "$dir"
    echo "$(date +%s) ${2:-step}" >> "$dir/marks"
    ;;
report)
    [ -f "$dir/pid" ] && kill "$(cat "$dir/pid")" 2>/dev/null
    peak=""
    [ -r "$cg/memory.peak" ] && peak=$(cat "$cg/memory.peak")
    [ -z "$peak" ] && [ -r "$cg/memory/memory.max_usage_in_bytes" ] && peak=$(cat "$cg/memory/memory.max_usage_in_bytes")
    limit=""
    [ -r "$cg/memory.max" ] && limit=$(cat "$cg/memory.max")
    python3 - "$dir" "${peak:-}" "${limit:-}" <<'EOF'
import sys
d, peak, limit = sys.argv[1], sys.argv[2], sys.argv[3]
gib = lambda b: f"{b/2**30:5.2f} GiB"
marks = []
for l in open(f"{d}/marks"):
    p = l.split(None, 1)
    if len(p) == 2:
        marks.append((int(p[0]), p[1].strip()))
samples = []
for l in open(f"{d}/samples"):
    p = l.split()
    if len(p) >= 5:
        samples.append((int(p[0]), int(p[1]), int(p[2]), int(p[3]) * 1024, p[4]))
end = samples[-1][0] if samples else marks[-1][0]
print("MEM-PROBE segment | wall | peak cgroup mem | peak anon | largest process")
tot = [0, 0, (0, "")]
for i, (t0, label) in enumerate(marks):
    t1 = marks[i + 1][0] if i + 1 < len(marks) else end + 1
    seg = [s for s in samples if t0 <= s[0] < t1]
    if not seg:
        continue
    cur = max(s[1] for s in seg)
    anon = max(s[2] for s in seg)
    top = max(seg, key=lambda s: s[3])
    tot[0] = max(tot[0], cur)
    tot[1] = max(tot[1], anon)
    if top[3] > tot[2][0]:
        tot[2] = (top[3], top[4])
    print(f"MEM-PROBE {label:28} | {t1 - t0:5d}s | {gib(cur)} | {gib(anon)} | {gib(top[3])} {top[4]}")
print(f"MEM-PROBE {'JOB TOTAL':28} | {end - marks[0][0]:5d}s | {gib(tot[0])} | {gib(tot[1])} | {gib(tot[2][0])} {tot[2][1]}")
if peak:
    print(f"MEM-PROBE cgroup memory.peak = {gib(int(peak))} (limit {limit or '?'})")
EOF
    ;;
*)
    echo "usage: $0 start | mark <label> | report" >&2
    exit 2
    ;;
esac
