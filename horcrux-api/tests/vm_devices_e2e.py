#!/usr/bin/env python3
"""Driven by vm_devices_e2e.sh: real QEMU, API calls checked against QMP and
the files QEMU creates."""
import json
import os
import socket
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
TOKEN = None
VM = "dev-e2e-vm"


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


def qmp(sock_path, command, arguments=None):
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock_path)
    f = s.makefile("rw")
    f.readline()
    f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
    f.flush()
    f.readline()
    msg = {"execute": command}
    if arguments:
        msg["arguments"] = arguments
    f.write(json.dumps(msg) + "\n")
    f.flush()
    while True:
        line = json.loads(f.readline())
        if "return" in line or "error" in line:
            s.close()
            return line


def peripherals(qsock):
    return {p["name"] for p in qmp(qsock, "qom-list", {"path": "/machine/peripheral"})["return"]}


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

call("POST", f"/vms/{VM}/stop", ok=None)
call("DELETE", f"/vms/{VM}", ok=None)
_, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": {"nets": [], "balloon": True, "guest_agent": True, "usb_controller": True},
})
check(vm["hardware"]["balloon"] and vm["hardware"]["guest_agent"] and vm["hardware"]["usb_controller"],
      "the VM keeps its device settings")
_, b = call("GET", f"/vms/{VM}/balloon")
check(b["enabled"] and not b["running"] and b["actual_mb"] is None and b["memory_mb"] == 128, "balloon info of a stopped VM")
call("POST", f"/vms/{VM}/balloon", {"target_mb": 64}, ok=(409,))
_, a = call("GET", f"/vms/{VM}/agent")
check(a["enabled"] and not a["running"] and not a["available"], "agent info of a stopped VM")
call("POST", f"/vms/{VM}/agent/ping", ok=(409,))

call("POST", f"/vms/{VM}/start")
_, vm = call("GET", f"/vms/{VM}")
disk = vm["disks"][0]["path"]
base = os.path.splitext(disk)[0]
qsock = base + ".qmp.sock"
peri = peripherals(qsock)
check({"balloon0", "hxusb", "qga"} <= peri, f"QEMU has the balloon, the USB controller and the agent port ({sorted(peri)})")
check(os.path.exists(base + ".qga.sock") and os.stat(base + ".qga.sock").st_mode & 0o170000 == 0o140000,
      "the guest agent's virtio-serial socket exists on the host")
chardev = qmp(qsock, "human-monitor-command", {"command-line": "info chardev"})["return"]
check("qga0" in chardev and ".qga.sock" in chardev, "QEMU's qga0 chardev listens on that socket")

# --- balloon ---------------------------------------------------------------
_, b = call("GET", f"/vms/{VM}/balloon")
check(b["enabled"] and b["running"] and b["actual_mb"] == 128, f"balloon: guest holds all 128 MB ({b})")
bal = qmp(qsock, "query-balloon")["return"]
check(bal["actual"] == 128 * 1024 * 1024, "QMP query-balloon agrees")
_, b = call("POST", f"/vms/{VM}/balloon", {"target_mb": 64})
check(b["target_mb"] == 64, "a balloon target is accepted by QEMU")
call("POST", f"/vms/{VM}/balloon", {"target_mb": 8}, ok=(422,))
call("POST", f"/vms/{VM}/balloon", {"target_mb": 4096}, ok=(422,))
check(call("GET", "/vms/nosuch-vm/balloon", ok=None)[0] == 404, "balloon of an unknown VM is 404")

# --- guest agent (no agent in this guest: the API says so plainly) -----------
t0 = time.time()
_, a = call("GET", f"/vms/{VM}/agent")
check(a["enabled"] and a["running"] and not a["available"] and "qemu-guest-agent" in (a["error"] or ""),
      f"agent info says the guest has no running agent ({a['error']})")
st, e = call("POST", f"/vms/{VM}/agent/ping", ok=None)
check(st == 503 and "qemu-guest-agent" in json.dumps(e), f"agent ping answers 503 with the reason ({st})")
call("POST", f"/vms/{VM}/agent/not-an-action", ok=(422,))
check(time.time() - t0 < 60, "agent calls time out quickly instead of hanging")

# --- USB: validation, and a passed-through device in the hardware ------------
st, hosts = call("GET", "/nodes/usb")
check(st == 200 and isinstance(hosts, list), f"host USB list answers ({len(hosts)} devices)")
call("POST", f"/vms/{VM}/usb", {"vendor_id": "dead", "product_id": "beef"}, ok=(404,))
call("POST", f"/vms/{VM}/usb", {"vendor_id": "xyz", "product_id": "1"}, ok=(422,))
call("DELETE", f"/vms/{VM}/usb/dead/beef", ok=(404,))

call("POST", f"/vms/{VM}/stop")
_, vm = call("GET", f"/vms/{VM}")
hw = vm["hardware"]
hw["usb"] = [{"vendor_id": "046D", "product_id": "C52B"}, {"vendor_id": "046d", "product_id": "c52b"}]
call("PUT", f"/vms/{VM}/config", {"hardware": hw}, ok=(422,))
hw["usb"] = [{"vendor_id": "046D", "product_id": "C52B"}]
_, vm = call("PUT", f"/vms/{VM}/config", {"hardware": hw})
call("POST", f"/vms/{VM}/start")
qsock = base + ".qmp.sock"
print("peripherals:", sorted(peripherals(qsock)), flush=True)
print(subprocess.run("ps -eo args | grep '[q]emu-system'", shell=True, capture_output=True, text=True).stdout[-1500:], flush=True)
print("vm hardware:", json.dumps(call("GET", f"/vms/{VM}")[1]["hardware"]), flush=True)
check("hxusb_046d_c52b" in peripherals(qsock), "a passed-through device is a usb-host device on the xHCI controller at start")
# a stopped VM's device can be removed through the API
call("POST", f"/vms/{VM}/stop")
_, vm = call("DELETE", f"/vms/{VM}/usb/046d/c52b")
check(vm["hardware"].get("usb", []) == [], "the device is removed from the VM's hardware")

# restart of the API keeps the settings
pid = open("/tmp/dev/api.pid").read().strip()
subprocess.run(["kill", pid])
time.sleep(2)
subprocess.run(["bash", "/tmp/dev/start-api.sh"], check=True)
_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")
_, vm = call("GET", f"/vms/{VM}")
check(vm["hardware"]["balloon"] and vm["hardware"]["guest_agent"], "device settings survive an API restart")
call("DELETE", f"/vms/{VM}", ok=None)
print("[vm-devices-e2e] ALL OK", flush=True)
