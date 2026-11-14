#!/usr/bin/env python3
"""Driven by nfs_storage_e2e.sh (phase `create` or `delete`): real API, a real
NFSv4 server (SRV_IP) and the kernel NFS client."""
import json
import os
import ssl
import sys
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
SRV = os.environ["SRV_IP"]
POOL_NAME = "e2e-nfs"
VOL = "e2e-vol"


def call(method, path, body=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=120) as r:
            status, text = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if ok is not None and status not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {status}: {text}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def pool():
    for p in call("GET", "/storage/pools")[1]:
        if p["name"] == POOL_NAME:
            return p
    return None


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

phase = sys.argv[1]
if phase == "create":
    # a path that is not an NFS export is refused up front
    st, _ = call("POST", "/storage/pools", {"name": "bad", "storage_type": "nfs", "path": "no-colon"}, ok=None)
    check(st >= 400, f"a malformed NFS path is refused (HTTP {st})")
    st, _ = call("POST", "/storage/pools", {"name": "bad2", "storage_type": "nfs", "path": f"{SRV}:/nope"}, ok=None)
    check(st >= 400, f"an export the server does not have is refused (HTTP {st})")
    check(pool() is None, "refused pools are not stored")

    _, p = call("POST", "/storage/pools", {"name": POOL_NAME, "storage_type": "nfs", "path": f"{SRV}:/export"})
    check(p["name"] == POOL_NAME, "NFS pool added by server:/export")
    p = pool()
    check(p["total"] > 0 and p["available"] > 0, f"pool reports the export's real capacity ({p['available']}/{p['total']} GB)")

    call("POST", f"/storage/pools/{p['id']}/volumes", {"name": VOL, "size": 1})
    names = [v["name"] for v in call("GET", f"/storage/pools/{p['id']}/volumes")[1]]
    check(any(VOL in n for n in names), f"volume listed through the API ({names})")
    st, _ = call("POST", f"/storage/pools/{p['id']}/volumes", {"name": VOL, "size": 1}, ok=None)
    check(st == 409 or st == 400, f"creating the same volume again is refused (HTTP {st})")
elif phase == "delete":
    p = pool()
    check(p is not None, "pool still registered")
    call("DELETE", f"/storage/pools/{p['id']}/volumes/{VOL}")
    names = [v["name"] for v in call("GET", f"/storage/pools/{p['id']}/volumes")[1]]
    check(not any(VOL in n for n in names), "volume gone from the API")
    call("DELETE", f"/storage/pools/{p['id']}")
    check(pool() is None, "pool removed")
else:
    raise SystemExit("phase must be create or delete")
