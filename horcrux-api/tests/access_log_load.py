#!/usr/bin/env python3
"""Access log load check: 10000 authenticated requests with the log off and
on (alternating rounds, 8 connections); the p99 latency with logging must not
exceed the p99 without it by more than MAX_P99_DELTA_MS (default 5 ms), and no
entry may be dropped. Against a live horcrux-api (BASE_URL, admin/admin).
"""
import http.client
import json
import os
import ssl
import statistics
import sys
import threading
import time
from urllib.parse import urlparse

BASE = os.environ.get("BASE_URL", "https://localhost:8006")
N = int(os.environ.get("LOAD_REQUESTS", "10000"))
THREADS = 8
ROUNDS = 3
MAX_DELTA = float(os.environ.get("MAX_P99_DELTA_MS", "5"))
_CA = os.environ.get("HORCRUX_CA_FILE")
CTX = ssl.create_default_context(cafile=_CA) if _CA else ssl._create_unverified_context()
U = urlparse(BASE)


def conn():
    return http.client.HTTPSConnection(U.hostname, U.port, context=CTX, timeout=60)


def call(c, method, path, tok=None, body=None):
    h = {"Content-Type": "application/json"}
    if tok:
        h["Authorization"] = f"Bearer {tok}"
    c.request(method, path, body=json.dumps(body) if body is not None else None, headers=h)
    r = c.getresponse()
    data = r.read()
    return r.status, (json.loads(data) if data else None)


def burst(tok, n):
    lat = []
    lock = threading.Lock()
    per = n // THREADS

    def worker():
        c = conn()
        mine = []
        for _ in range(per):
            t = time.perf_counter()
            c.request("GET", "/api/health", headers={"Authorization": f"Bearer {tok}"})
            r = c.getresponse()
            r.read()
            mine.append((time.perf_counter() - t) * 1000)
            assert r.status == 200, r.status
        with lock:
            lat.extend(mine)

    ts = [threading.Thread(target=worker) for _ in range(THREADS)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    lat.sort()
    return lat[int(len(lat) * 0.99) - 1], statistics.median(lat)


def main():
    c = conn()
    _, v = call(c, "POST", "/api/auth/login", body={"username": "admin", "password": "admin"})
    tok = v["ticket"]
    _, s = call(c, "GET", "/api/access-log/settings", tok)
    off, on = [], []
    try:
        burst(tok, 800)  # warm up
        for i in range(ROUNDS):
            for mode in ("off", "on"):
                s["enabled"] = mode == "on"
                st, _ = call(c, "PUT", "/api/access-log/settings", tok, s)
                assert st == 200, st
                p99, p50 = burst(tok, N // ROUNDS)
                (on if mode == "on" else off).append(p99)
                print(f"round {i} log {mode}: p50 {p50:.2f} ms p99 {p99:.2f} ms")
    finally:
        s["enabled"] = True
        call(c, "PUT", "/api/access-log/settings", tok, s)
    delta = statistics.median(on) - statistics.median(off)
    _, st = call(c, "GET", "/api/access-log/stats", tok)
    col = st["collector"]
    print(f"p99 off {statistics.median(off):.2f} ms, on {statistics.median(on):.2f} ms, delta {delta:.2f} ms; "
          f"recorded {col['written']}, dropped {col['dropped']}, write errors {col['write_errors']}")
    assert col["dropped"] == 0, "entries were dropped"
    assert col["write_errors"] == 0
    assert delta <= MAX_DELTA, f"logging raised p99 by {delta:.2f} ms (limit {MAX_DELTA})"
    print("ok - access log load check")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"FAIL: {e}")
        sys.exit(1)
