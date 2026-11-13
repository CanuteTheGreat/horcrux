#!/usr/bin/env python3
"""Driven by snapshot_schedule_e2e.sh: real API, real qcow2 disk, results read
back with `qemu-img snapshot -l`."""
import json
import ssl
import subprocess
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
VM = "snapsched-e2e"


def call(method, path, body=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=180) as r:
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


def disk_snapshots(disk):
    out = subprocess.run(["qemu-img", "snapshot", "-l", disk], capture_output=True, text=True, check=True).stdout
    names = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) > 2 and parts[0].isdigit():
            names.append(parts[1])
    return names


def api_snaps():
    return [s["name"] for s in call("GET", f"/vms/{VM}/snapshots")[1]]


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

call("DELETE", f"/vms/{VM}", ok=None)
_, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": {"nets": []},
})
disk = call("GET", f"/vms/{VM}")[1]["disks"][0]["path"]
check(disk_snapshots(disk) == [], "the VM's qcow2 starts with no snapshots")

call("POST", f"/vms/{VM}/snapshots", {"name": "nightly-before-upgrade", "description": "by hand", "include_memory": False})
check(any(n.startswith("nightly-before-upgrade") for n in disk_snapshots(disk)), "a hand-made snapshot exists in the image")

sched = {"vm_id": VM, "name": "nightly", "frequency": {"custom": {"cron": "* * * * *"}},
         "retention_count": 2, "enabled": True, "include_memory": False}
st, _ = call("POST", "/snapshot-schedules", dict(sched, name="bad", frequency={"custom": {"cron": "61 * * * *"}}), ok=None)
check(st == 422, f"an invalid cron expression is refused with 422 ({st})")
_, s = call("POST", "/snapshot-schedules", sched)
sid = s["id"]
check(s["next_run"] > time.time() - 5 and s["next_run"] - time.time() <= 61, "next_run is within the next minute")

# The scheduler wakes every minute; three runs prove retention (keep 2).
seen = set()
deadline = time.time() + 330
while time.time() < deadline:
    _, cur = call("GET", f"/snapshot-schedules/{sid}")
    if cur.get("last_run"):
        seen.add(cur["last_run"])
    if len(seen) >= 3:
        break
    time.sleep(5)
check(len(seen) >= 3, f"the schedule ran by itself {len(seen)} times (last_run changed)")

mine = [n for n in api_snaps() if n.startswith("nightly_")]
check(len(mine) == 2, f"retention keeps exactly 2 scheduled snapshots ({mine})")
check("nightly-before-upgrade" in api_snaps(), "retention left the hand-made 'nightly-before-upgrade' snapshot alone")
img = disk_snapshots(disk)
check(len([n for n in img if n.startswith("nightly_")]) == 2 and any(n.startswith("nightly-before-upgrade") for n in img),
      f"qemu-img agrees: 2 scheduled + the hand-made snapshot ({img})")

# disabled: nothing new
call("PUT", f"/snapshot-schedules/{sid}", dict(sched, enabled=False))
before = sorted(api_snaps())
time.sleep(130)
check(sorted(api_snaps()) == before, "a disabled schedule makes no more snapshots")

# delete
call("DELETE", f"/snapshot-schedules/{sid}")
st, _ = call("GET", f"/snapshot-schedules/{sid}", ok=None)
check(st == 404, "a deleted schedule is gone (404)")
check(all(x["id"] != sid for x in call("GET", "/snapshot-schedules")[1]), "and not in the list")
call("DELETE", f"/vms/{VM}", ok=None)
print("snapshot schedule e2e: all checks passed")
