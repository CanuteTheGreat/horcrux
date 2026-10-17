#!/usr/bin/env python3
"""Mutation-score floors for cargo-mutants (weekly mutation workflow).

Usage: mutants_floor.py <mutants.out dir> <floors.json> [group]

With a group, only that group is scored (one CI job per group).

floors.json maps a module group name to {"files": [...], "floor": <percent>}.
For each group the score is caught / (caught + missed) over the mutants in
its files (timeouts count as caught, unviable mutants are ignored). The job
fails when a group drops below its floor. Floors only ever go up: when a
group beats its floor by 2+ points the script prints the new value to put
in floors.json, and every missed mutant is listed (each one is a missing
assertion).
"""
import json
import sys
from collections import defaultdict


def main() -> int:
    out_dir, floors_path = sys.argv[1], sys.argv[2]
    with open(f"{out_dir}/outcomes.json") as f:
        outcomes = json.load(f)
    with open(floors_path) as f:
        floors = json.load(f)
    if len(sys.argv) > 3:
        floors = {sys.argv[3]: floors[sys.argv[3]]}

    stats = defaultdict(lambda: {"caught": 0, "missed": 0, "timeout": 0, "unviable": 0})
    missed = defaultdict(list)
    for o in outcomes.get("outcomes", []):
        sc = o.get("scenario")
        if not isinstance(sc, dict) or "Mutant" not in sc:
            continue
        m = sc["Mutant"]
        path = m.get("file", "")
        summary = o.get("summary", "")
        group = next(
            (g for g, spec in floors.items() if any(path.endswith(f) for f in spec["files"])),
            None,
        )
        if group is None:
            continue
        key = {
            "CaughtMutant": "caught",
            "MissedMutant": "missed",
            "Timeout": "timeout",
            "Unviable": "unviable",
        }.get(summary)
        if key is None:
            continue
        stats[group][key] += 1
        if key == "missed":
            span = m.get("span", {}).get("start", {})
            missed[group].append(
                f"{path}:{span.get('line', '?')}: {m.get('name') or m.get('replacement', '')}"
            )

    failed = False
    print(f"{'group':<14}{'caught':>8}{'missed':>8}{'timeout':>9}{'unviable':>10}{'score':>8}{'floor':>7}")
    for group, spec in floors.items():
        s = stats[group]
        denom = s["caught"] + s["timeout"] + s["missed"]
        score = 100.0 * (s["caught"] + s["timeout"]) / denom if denom else 0.0
        floor = float(spec["floor"])
        print(
            f"{group:<14}{s['caught']:>8}{s['missed']:>8}{s['timeout']:>9}{s['unviable']:>10}"
            f"{score:>7.1f}%{floor:>6.0f}%"
        )
        if denom == 0:
            print(f"::error::{group}: no mutants were tested - the file list is stale")
            failed = True
        elif score < floor:
            print(f"::error::{group}: mutation score {score:.1f}% below floor {floor:.0f}%")
            failed = True
        elif score >= floor + 2:
            print(f"::notice::{group}: raise the floor to {int(score)} in {floors_path}")
    for group, items in missed.items():
        print(f"\nMissed mutants in {group} ({len(items)}): each is an untested behaviour")
        for i in items:
            print(f"  {i}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
