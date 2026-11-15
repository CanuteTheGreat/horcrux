#!/usr/bin/env python3
"""Driven by iscsi_storage_e2e.sh (phase `create` or `delete`): real API, a real
iSCSI target (SRV_IP, IQN) and the kernel initiator."""
import json
import os
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

PORT = os.environ["PORT"]
API = f"https://127.0.0.1:{PORT}/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = ""
SRV = os.environ["SRV_IP"]
IQN = os.environ["IQN"]
POOL_NAME = "e2e-iscsi"
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


def sessions():
    r = subprocess.run(["iscsiadm", "-m", "session"], capture_output=True, text=True)
    return r.stdout


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

phase = sys.argv[1]
if phase == "create":
    st, _ = call("POST", "/storage/pools", {"name": "bad", "storage_type": "iscsi", "path": "not-an-iscsi-path"}, ok=None)
    check(st >= 400, f"a malformed iSCSI path is refused (HTTP {st})")
    st, _ = call("POST", "/storage/pools", {"name": "bad2", "storage_type": "iscsi", "path": f"iscsi://{SRV}:3260/iqn.2026-11.test.horcrux:nope"}, ok=None)
    check(st >= 400, f"a target the portal does not offer is refused (HTTP {st})")
    check(pool() is None, "refused pools are not stored")
    check(IQN not in sessions(), "refused pools did not log in")

    _, p = call("POST", "/storage/pools", {"name": POOL_NAME, "storage_type": "iscsi", "path": f"iscsi://{SRV}:3260/{IQN}"})
    check(p["name"] == POOL_NAME, "iSCSI pool added by iscsi://portal/iqn")
    check(IQN in sessions(), "adding the pool logged in to the target")
    p = pool()
    check(p["total"] == 3, f"pool reports the LUNs' real capacity ({p['total']} GB, expected 3)")

    vols = {v["name"]: v for v in call("GET", f"/storage/pools/{p['id']}/volumes")[1]}
    check(set(vols) == {"lun1", "lun2"}, f"the target's LUNs are listed as volumes ({sorted(vols)})")
    check(vols["lun1"]["size_bytes"] == 1 * GIB and vols["lun2"]["size_bytes"] == 2 * GIB, "volume sizes match the LUNs")
    dev = vols["lun1"]["path"]
    check(os.path.exists(dev), f"volume path {dev} is a block device")

    # the info command proves qemu can open the volume as a raw disk
    info = subprocess.run(["qemu-img", "info", "--output=json", "-f", "raw", dev], capture_output=True, text=True)
    check(info.returncode == 0 and json.loads(info.stdout)["virtual-size"] == GIB, "qemu-img opens the volume as a 1 GiB raw disk")

    with open(dev, "r+b", buffering=0) as f:
        f.write(b"HORCRUX-ISCSI-E2E")
        os.fsync(f.fileno())
    check(True, "wrote a pattern through the volume's device")
elif phase == "delete":
    p = pool()
    check(p is not None, "pool still registered")
    call("DELETE", f"/storage/pools/{p['id']}")
    check(pool() is None, "pool removed")
    for _ in range(20):
        if IQN not in sessions():
            break
        time.sleep(0.5)
else:
    raise SystemExit("phase must be create or delete")
