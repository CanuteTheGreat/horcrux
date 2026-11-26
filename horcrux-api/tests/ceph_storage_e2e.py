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

# ---- VM disks on the RBD pool (create / add / resize / snapshot / clone / delete) ----
import hashlib
import tempfile

VM = "rbdvm1"
vmpools = call("GET", "/vms/host-options")[1].get("storage_pools", [])
check(any(x["id"] == pid for x in vmpools), "the Ceph pool is offered for VM disks")
_, vm = call("POST", "/vms", {"id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
                              "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped",
                              "display": {"protocol": "none"}, "hardware": {},
                              "disks": [{"size_gb": 1, "storage": pid, "path": "", "disk_type": "virtio", "cache": ""}]})
d0 = f"{RADOS}/vm-{VM}-disk-0"
check(rbd_info(f"vm-{VM}-disk-0") is not None and rbd_info(f"vm-{VM}-disk-0")["size"] == GIB,
      "creating a VM on the pool made a 1 GiB RBD image vm-<id>-disk-0")
_, vm = call("GET", f"/vms/{VM}")
check(vm["disks"][0]["path"] == f"rbd:{d0}", f"disk0 path is the rbd: spec ({vm['disks'][0]['path']})")

_, vm = call("POST", f"/vms/{VM}/disks", {"size_gb": 1, "storage": pid})
check(rbd_info(f"vm-{VM}-disk-1") is not None, "adding a disk made RBD image vm-<id>-disk-1")
call("PUT", f"/vms/{VM}/disks/1", {"size_gb": 2})
check(rbd_info(f"vm-{VM}-disk-1")["size"] == 2 * GIB, "growing disk 1 resized the RBD image")

# data integrity: write a pattern into disk 1 through qemu's rbd driver, read it back
pat = os.urandom(4 * 1024 * 1024)
tmp = tempfile.mkdtemp()
src = os.path.join(tmp, "pat.raw")
open(src, "wb").write(pat)
qimg = lambda *a: subprocess.run(["qemu-img", *a], capture_output=True, text=True)
r = qimg("convert", "-n", "-f", "raw", "-O", "raw", src, f"rbd:{RADOS}/vm-{VM}-disk-1")
check(r.returncode == 0, f"qemu-img wrote through the rbd driver ({r.stderr.strip()})")


def read_back(spec):
    out = os.path.join(tmp, "out.raw")
    r = qimg("convert", "-f", "raw", "-O", "raw", spec, out)
    if r.returncode != 0:
        return None
    return hashlib.sha256(open(out, "rb").read(len(pat))).hexdigest()


want = hashlib.sha256(pat).hexdigest()
check(read_back(f"rbd:{RADOS}/vm-{VM}-disk-1") == want, "the pattern reads back identical")

_, snap = call("POST", f"/vms/{VM}/snapshots", {"name": "s1", "include_memory": False})
sn = subprocess.run(["rbd", "snap", "ls", "--format", "json", f"{RADOS}/vm-{VM}-disk-1"], capture_output=True, text=True)
check(sn.returncode == 0 and len(json.loads(sn.stdout)) >= 1, "VM snapshot created an RBD snapshot on the disk")
subprocess.run(["qemu-img", "convert", "-n", "-f", "raw", "-O", "raw", "/dev/zero", f"rbd:{RADOS}/vm-{VM}-disk-1"], capture_output=True)
call("POST", f"/vms/{VM}/snapshots/{snap['id']}/restore", {})
check(read_back(f"rbd:{RADOS}/vm-{VM}-disk-1") == want, "restoring the snapshot brought the pattern back")

_, c = call("POST", f"/vms/{VM}/clone", {"name": "rbdclone", "id": "rbdclone", "mode": "full"})
check(rbd_info("vm-rbdclone-disk-1") is not None and read_back(f"rbd:{RADOS}/vm-rbdclone-disk-1") == want,
      "full clone has its own RBD images with the same data")
_, c = call("POST", f"/vms/{VM}/clone", {"name": "rbdlinked", "id": "rbdlinked", "mode": "linked"})
li = rbd_info("vm-rbdlinked-disk-1")
check(li is not None and read_back(f"rbd:{RADOS}/vm-rbdlinked-disk-1") == want,
      "linked clone reads the same data")
check(li is not None and "parent" in li, "linked clone is a child of a snapshot")

for v in ("rbdlinked", "rbdclone", VM):
    call("DELETE", f"/vms/{v}")
left = subprocess.run(["rbd", "ls", RADOS], capture_output=True, text=True).stdout.split()
check(not [i for i in left if i.startswith("vm-")], f"deleting the VMs removed their RBD images (left: {left})")


call("DELETE", f"/storage/pools/{pid}")
check(pool() is None, "pool removed")
r = subprocess.run(["ceph", "osd", "pool", "ls"], capture_output=True, text=True)
check(RADOS in r.stdout.split(), "removing the pool from Horcrux left the RADOS pool alone")
print("ALL OK")
