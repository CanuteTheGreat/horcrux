#!/usr/bin/env python3
"""Merge the outcomes of a sharded cargo-mutants full pass (`--shard i/N`) into one mutants.out directory.

Usage: mutants_merge.py <shards dir with 0/ .. N-1/ each holding outcomes.json> <out mutants.out dir> <N>

Every shard must be present and every mutant appears in exactly one shard (cargo-mutants partitions the
list); a missing shard fails the merge, so a floor is never applied to a partial pass.
"""
import json
import os
import sys


def main() -> int:
    src, out, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
    merged, missed, seen = [], [], set()
    for i in range(n):
        d = os.path.join(src, str(i))
        path = os.path.join(d, "outcomes.json")
        if not os.path.isfile(path):
            print(f"::error::shard {i}/{n}: {path} is missing - the pass is incomplete")
            return 1
        with open(path) as f:
            data = json.load(f)
        for o in data.get("outcomes", []):
            sc = o.get("scenario")
            if isinstance(sc, dict) and "Mutant" in sc:
                key = json.dumps(sc["Mutant"], sort_keys=True)
                if key in seen:
                    print(f"::error::shard {i}/{n}: mutant listed twice: {sc['Mutant'].get('name')}")
                    return 1
                seen.add(key)
            merged.append(o)
        mp = os.path.join(d, "missed.txt")
        if os.path.isfile(mp):
            missed += open(mp).read().splitlines()
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "outcomes.json"), "w") as f:
        json.dump({"outcomes": merged, "total_mutants": len(seen)}, f)
    with open(os.path.join(out, "missed.txt"), "w") as f:
        f.write("\n".join(missed) + ("\n" if missed else ""))
    print(f"merged {n} shards: {len(seen)} mutants, {len(merged)} outcomes, {len(missed)} missed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
