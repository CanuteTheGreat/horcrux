#!/usr/bin/env python3
"""Driven by ceph_storage_e2e.sh: real API next to a real single-node Ceph cluster
(RADOS pool hxpool) with the ceph and rbd CLIs."""
import json
import os
import ssl
import subprocess
import urllib.error
import urllib.request

PORT = os.environ["PORT"]
API = f"https://127.0.0.1:{PORT}/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = ""
POOL_NAME = "e2e-ceph"
RADOS = "hxpool"
GIB = 1024 ** 3


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


def rbd_info(image):
    r = subprocess.run(["rbd", "info", "--format", "json", f"{RADOS}/{image}"], capture_output=True, text=True)
    return json.loads(r.stdout) if r.returncode == 0 else None


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

st, _ = call("POST", "/storage/pools", {"name": "bad", "storage_type": "ceph", "path": "no-such-rados-pool"}, ok=None)
check(st >= 400, f"a RADOS pool the cluster does not have is refused (HTTP {st})")
check(pool() is None, "the refused pool is not stored")

_, p = call("POST", "/storage/pools", {"name": POOL_NAME, "storage_type": "ceph", "path": RADOS})
check(p["name"] == POOL_NAME, "Ceph pool added by RADOS pool name")
p = pool()
check(p["total"] >= 2 and p["available"] >= 2, f"pool reports the cluster's real capacity ({p['available']}/{p['total']} GB, OSD is 3 GiB)")
pid = p["id"]

call("POST", f"/storage/pools/{pid}/volumes", {"name": "vol1", "size": 1})
info = rbd_info("vol1")
check(info is not None and info["size"] == GIB, "creating a volume made a 1 GiB RBD image in the cluster")
vols = {v["name"]: v for v in call("GET", f"/storage/pools/{pid}/volumes")[1]}
check("vol1" in vols and vols["vol1"]["size_bytes"] == GIB, f"the image is listed with its size ({sorted(vols)})")
check(vols["vol1"]["path"] == f"rbd:{RADOS}/vol1", "the volume path is the rbd: URL qemu opens")

st, _ = call("POST", f"/storage/pools/{pid}/volumes", {"name": "vol1", "size": 1}, ok=None)
check(st in (400, 409), f"creating the same volume again is refused (HTTP {st})")
st, _ = call("POST", f"/storage/pools/{pid}/volumes", {"name": "huge", "size": 500}, ok=None)
check(st >= 400 and rbd_info("huge") is None, f"a volume larger than the pool's free space is refused (HTTP {st})")

call("POST", f"/storage/pools/{pid}/volumes/vol1/resize", {"size": 2})
check(rbd_info("vol1")["size"] == 2 * GIB, "growing the volume to 2 GiB resized the RBD image")
st, _ = call("POST", f"/storage/pools/{pid}/volumes/vol1/resize", {"size": 1}, ok=None)
check(st == 400 and rbd_info("vol1")["size"] == 2 * GIB, f"shrinking is refused and the image is untouched (HTTP {st})")

# data written through rbd is visible as volume usage: the pool's free space moves
subprocess.run(["rbd", "bench", "--io-type", "write", "--io-total", "256M", "--io-size", "4M", "--io-pattern", "seq", f"{RADOS}/vol1"],
               capture_output=True, check=True)
call("DELETE", f"/storage/pools/{pid}/volumes/vol1")
check(rbd_info("vol1") is None, "deleting the volume removed the RBD image")
names = [v["name"] for v in call("GET", f"/storage/pools/{pid}/volumes")[1]]
check("vol1" not in names, "the volume is gone from the API")

call("DELETE", f"/storage/pools/{pid}")
check(pool() is None, "pool removed")
r = subprocess.run(["ceph", "osd", "pool", "ls"], capture_output=True, text=True)
check(RADOS in r.stdout.split(), "removing the pool from Horcrux left the RADOS pool alone")
print("ALL OK")
