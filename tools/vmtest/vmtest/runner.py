"""Test runner: `python3 -m vmtest run scenarios/<file>.py [--out DIR] [--arg k=v ...]`.

A scenario module exposes  `def define(args) -> vmtest.Test`.  Exit codes: 0 pass, 1 test failed,
2 harness/usage error, 124 hard deadline hit.
"""
from __future__ import annotations

import argparse
import importlib.util
import os
import signal
import sys
import threading
import time
import traceback

from .machines import Machines
from .qemu import kill_all


def load(path):
    spec = importlib.util.spec_from_file_location("vmtest_scenario", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def run_test(test, out, args=None, keep_disks=False, slow=0):
    m = Machines(test, out, args, slow)
    ok, err = False, ""
    deadline = threading.Timer(test.timeout * m.slow, lambda: (print(f"[FAIL] hard deadline {test.timeout * m.slow:.0f}s hit: killing every VM", flush=True),
                                                              kill_all(), os._exit(124)))
    deadline.daemon = True
    deadline.start()
    signal.signal(signal.SIGTERM, lambda *a: (kill_all(), os._exit(143)))
    try:
        print(f"vmtest: {test.name}  accel={'kvm' if m.kvm else 'tcg'} slow={m.slow} out={m.out}", flush=True)
        test.run(m)
        ok = all(s["ok"] or s.get("advisory") for s in m.steps)
        if not ok:
            err = "failed steps: " + ", ".join(s["step"] for s in m.steps if not s["ok"] and not s.get("advisory"))
    except BaseException as e:   # noqa: BLE001 - evidence first, then report
        err = f"{type(e).__name__}: {e}"
        print(f"[FAIL] {err}", flush=True)
        traceback.print_exc()
    finally:
        deadline.cancel()
        if not ok:
            m.collect()
            for n in m.nodes.values():
                if n.q:
                    print(f"--- {n.name} serial tail ---\n{n.q.serial_tail(1200)}", flush=True)
        m.result(ok, err)
        m.teardown(keep_disks)
    print(("ALL PASS " if ok else "FAILED ") + test.name + (f" ({err})" if err else ""), flush=True)
    return 0 if ok else 1


def main(argv=None):
    ap = argparse.ArgumentParser(prog="vmtest")
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("scenario")
    r.add_argument("--out", default="vmtest-out")
    r.add_argument("--arg", action="append", default=[], help="k=v passed to define(args)")
    r.add_argument("--keep-disks", action="store_true")
    r.add_argument("--slow", type=float, default=0)
    a = ap.parse_args(argv)
    args = dict(kv.split("=", 1) for kv in a.arg)
    try:
        test = load(a.scenario).define(args)
    except Exception as e:   # noqa: BLE001
        traceback.print_exc()
        print(f"vmtest: cannot build the scenario: {e}", file=sys.stderr)
        return 2
    return run_test(test, a.out, args, a.keep_disks, a.slow)
