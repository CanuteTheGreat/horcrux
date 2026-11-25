#!/usr/bin/env python3
"""Driven by storage_plugin_e2e.sh (runs in the API's container, next to a real
SSH server at $SRV_IP). Steps: pool | volume-cleanup | vm | teardown; state
between steps in /tmp/sp-e2e.json."""
import json
import os
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
STATE = "/tmp/sp-e2e.json"
SRV = os.environ["SRV_IP"]
TOKEN = None
GIB = 1024 ** 3


def call(method, path, body=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=300) as r:
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


def state():
    return json.load(open(STATE)) if os.path.exists(STATE) else {}


def save(**kw):
    s = state()
    s.update(kw)
    json.dump(s, open(STATE, "w"))


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")
step = sys.argv[1]

if step == "pool":
    _, l = call("GET", "/storage/plugins")
    p = next((p for p in l["plugins"] if p["id"] == "sshfs"), None)
    check(p is not None and p["usable"], f"the installed sshfs plugin is listed and usable ({p})")
    schema = {f["key"]: f for f in p["description"]["schema"]}
    check(schema["port"]["max"] == 65535 and schema["port"]["hint"], "its schema carries limits and hints")
    check(schema["password"]["type"] == "secret", "the password field is a secret")
    cfg = {"host": SRV, "remote_dir": "/srv/hx", "identity_file": "/root/.ssh/id_ed25519"}
    st, e = call("POST", "/storage/pools", {"name": "sp", "storage_type": "plugin:sshfs",
                                             "config": dict(cfg, port=70000)}, ok=None)
    check(st == 400 and "port" in json.dumps(e) and "65535" in json.dumps(e),
          f"an out-of-range port is refused with 400 naming the field ({st} {e})")
    st, e = call("POST", "/storage/pools", {"name": "sp", "storage_type": "plugin:sshfs",
                                             "config": dict(cfg, auth="password")}, ok=None)
    check(st == 400 and "password" in json.dumps(e), "a password login without a password is refused")
    _, pool = call("POST", "/storage/pools", {"name": "sp", "storage_type": "plugin:sshfs", "config": cfg})
    pid = pool["id"]
    save(pid=pid)
    check(pool["storage_type"] == "plugin:sshfs" and pool["plugin_config"]["port"] == 22,
          "the pool is created with defaults filled in")
    mounts = open("/proc/mounts").read()
    check(pool["path"] and f" {pool['path']} fuse.sshfs " in mounts, f"the plugin mounted the export at {pool['path']}")
    check(pool["total"] > 0, f"capacity comes from the server ({pool['total']} GB)")
    _, v = call("POST", f"/storage/pools/{pid}/volumes", {"name": "e2e-vol", "size": 1})
    check(v["size_bytes"] == GIB and v["format"] == "qcow2", "a 1 GiB qcow2 volume is created")
    _, vols = call("GET", f"/storage/pools/{pid}/volumes")
    check([x["name"] for x in vols] == ["e2e-vol"], "it is listed")
    call("POST", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots", {"name": "before-grow"})
    _, snaps = call("GET", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots")
    check(snaps["snapshots"] == ["before-grow"], "a snapshot is taken and listed")
    st, _ = call("POST", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots", {"name": "before-grow"}, ok=None)
    check(st == 409, "a duplicate snapshot is a conflict")
    _, v = call("POST", f"/storage/pools/{pid}/volumes/e2e-vol/resize", {"size": 2})
    check(v["size_bytes"] == 2 * GIB, "the volume grows to 2 GiB")
    st, _ = call("POST", f"/storage/pools/{pid}/volumes/e2e-vol/resize", {"size": 1}, ok=None)
    check(st == 400, "shrinking is refused")

elif step == "volume-cleanup":
    pid = state()["pid"]
    # rollback brings back the pre-grow size, then the snapshot and volume go
    call("POST", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots/before-grow/rollback")
    _, vols = call("GET", f"/storage/pools/{pid}/volumes")
    check(vols[0]["size_bytes"] == GIB, f"rollback restored the 1 GiB volume ({vols[0]['size_bytes']})")
    call("DELETE", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots/before-grow")
    _, snaps = call("GET", f"/storage/pools/{pid}/volumes/e2e-vol/snapshots")
    check(snaps["snapshots"] == [], "the snapshot is deleted")
    call("DELETE", f"/storage/pools/{pid}/volumes/e2e-vol")
    _, vols = call("GET", f"/storage/pools/{pid}/volumes")
    check(vols == [], "the volume is deleted")

elif step == "vm":
    pid = state()["pid"]
    _, opts = call("GET", "/vms/host-options")
    check(any(p["id"] == pid for p in opts.get("storage_pools", [])),
          "the plugin pool is offered for VM disks")
    call("POST", "/vms/e2e-boot/stop", ok=None)
    call("DELETE", "/vms/e2e-boot", ok=None)
    _, vm = call("POST", "/vms", {
        "id": "e2e-boot", "name": "e2e-boot", "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
        "disks": [{"path": "", "size_gb": 1, "disk_type": "virtio", "cache": "", "storage": pid}],
        "display": {"protocol": "none", "video": "std", "sound": "none"},
        "hardware": {"nets": []},
    })
    disk = vm["disks"][0]["path"]
    _, pool = call("GET", f"/storage/pools/{pid}")
    check(disk.startswith(pool["path"] + "/") and os.path.exists(disk), f"the VM disk is on the plugin pool ({disk})")
    # a boot sector that prints a marker on COM1 and halts
    code = bytes.fromhex("fc31c08ed8baf803be167cac84c07403eeebf8f4ebfd") + b"HXBOOT-OK\r\n\0"
    sector = code + b"\0" * (510 - len(code)) + b"\x55\xaa"
    open("/tmp/boot.raw", "wb").write(sector + b"\0" * (GIB // 1024 - 512))
    subprocess.run(["qemu-img", "convert", "-f", "raw", "-O", "qcow2", "/tmp/boot.raw", disk], check=True)
    call("POST", "/vms/e2e-boot/start")
    log = os.path.splitext(disk)[0] + ".serial.log"
    deadline, text = time.time() + 120, ""
    while time.time() < deadline:
        text = open(log, errors="replace").read() if os.path.exists(log) else ""
        if "HXBOOT-OK" in text:
            break
        time.sleep(2)
    check("HXBOOT-OK" in text, f"the VM booted from its disk on the plugin pool (serial: {text[-200:]!r})")
    _, v = call("GET", "/vms/e2e-boot")
    check(v["status"] == "running", "the VM is running")
    call("POST", "/vms/e2e-boot/stop")
    save(vm_disk=disk)

elif step == "teardown":
    s = state()
    call("DELETE", "/vms/e2e-boot", ok=None)
    st, _ = call("DELETE", f"/storage/pools/{s['pid']}", ok=None)
    check(st == 200, "the pool is removed")
    _, l = call("GET", "/storage/pools")
    check(all(p["id"] != s["pid"] for p in l), "it is gone from the list")
else:
    raise SystemExit(f"unknown step {step}")
