#!/usr/bin/env python3
"""Driven by vm_nic_e2e.sh inside a NET_ADMIN container with the API on :8006.

Every step goes through the HTTP API (what the UI calls) and is checked
against the kernel (ip/bridge/tcpdump) or the running QEMU (QMP).
"""
import http.server
import json
import os
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
BR = "vmbr-e2e"
VM = "nic-e2e-vm"


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


def sh(cmd):
    return subprocess.run(cmd, shell=True, check=True, capture_output=True, text=True).stdout


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def login():
    global TOKEN
    TOKEN = None
    _, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN = r.get("ticket") or r.get("token")


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


def bridge_ports(br):
    return [l["ifname"] for l in json.loads(sh(f"ip -j link show master {br}") or "[]")]


def restart_api():
    pid = open("/tmp/nic/api.pid").read().strip()
    subprocess.run(["kill", pid])
    for _ in range(50):
        if not os.path.exists(f"/proc/{pid}"):
            break
        time.sleep(0.2)
    sh("bash /tmp/nic/start-api.sh")
    login()


login()

# --- Host Bridges ----------------------------------------------------------
call("DELETE", f"/network/bridges/{BR}", ok=None)
call("POST", "/network/bridges",
     {"name": BR, "vlan_aware": True, "address": "10.77.0.1/24", "ports": []})
link = json.loads(sh(f"ip -d -j link show {BR}"))[0]
check(link["linkinfo"]["info_kind"] == "bridge"
      and link["linkinfo"]["info_data"]["vlan_filtering"] == 1,
      "API created a VLAN-aware Linux bridge")
check("10.77.0.1/24" in sh(f"ip -o addr show dev {BR}").replace(" ", " "),
      "bridge carries the requested host address")
_, lst = call("GET", "/network/bridges")
check(any(b["name"] == BR and b["managed"] and b["vlan_aware"] for b in lst),
      "GET /network/bridges lists it as managed + VLAN-aware")
status, body = call("POST", "/network/bridges",
                    {"name": "vmbr-e2e2", "ports": ["eth0"]}, ok=None)
check(status == 400 and "cut the host" in json.dumps(body),
      "a port carrying the host's address is refused (would cut the host off)")
_, opts = call("GET", "/vms/host-options")
check(any(b["name"] == BR and b["vlan_aware"] for b in opts["bridges"]),
      "VM host options offer the bridge")

# Managed bridges come back after a host reboot (API restart re-creates).
call("POST", "/network/bridges", {"name": "vmbr-e2e3", "vlan_aware": False})
sh("ip link del vmbr-e2e3")
restart_api()
for _ in range(30):
    if os.path.exists("/sys/class/net/vmbr-e2e3/bridge"):
        break
    time.sleep(0.5)
check(os.path.exists("/sys/class/net/vmbr-e2e3/bridge"),
      "managed bridge re-created when the API starts")
call("DELETE", "/network/bridges/vmbr-e2e3")
check(not os.path.exists("/sys/class/net/vmbr-e2e3"), "DELETE removes the bridge")

# --- ISO library: download from a URL ---------------------------------------
os.makedirs("/tmp/isosrc/d", exist_ok=True)
open("/tmp/isosrc/d/hello.txt", "w").write("horcrux e2e\n")
sh("genisoimage -quiet -V E2E -o /tmp/isosrc/e2e.iso /tmp/isosrc/d")
handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory="/tmp/isosrc", **k)
# Served on the bridge's address: the API refuses to fetch from the node's
# own loopback (or cloud metadata), which this checks first.
srv = http.server.ThreadingHTTPServer(("10.77.0.1", 0), handler)
threading.Thread(target=srv.serve_forever, daemon=True).start()
status, body = call("POST", "/storage/isos/download",
                    {"url": f"http://127.0.0.1:{srv.server_address[1]}/e2e.iso"}, ok=None)
check(status == 403 and "refusing" in json.dumps(body),
      "ISO download from the node's own loopback is refused (403)")
call("POST", "/storage/isos/download",
     {"url": f"http://10.77.0.1:{srv.server_address[1]}/e2e.iso"})
iso = None
for _ in range(60):
    _, isos = call("GET", "/storage/isos")
    iso = next((i for i in isos if i["name"] == "e2e.iso" and not i["downloading"]), None)
    if iso:
        break
    time.sleep(0.5)
check(iso and iso["size"] == os.path.getsize("/tmp/isosrc/e2e.iso"),
      "ISO downloaded into the library")

# --- VM with a NIC on the bridge, VLAN 20, PXE boot --------------------------
call("POST", f"/vms/{VM}/stop", ok=None)
call("DELETE", f"/vms/{VM}", ok=None)
_, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": {"nets": [{"model": "virtio", "bridge": BR, "vlan": 20}],
                 "boot": ["net", "disk"]},
})
mac = vm["hardware"]["nets"][0]["mac"]
check(mac and int(mac[:2], 16) & 2 == 2, f"locally administered MAC generated ({mac})")
sh(f"bridge vlan add dev {BR} vid 20 self")
cap = subprocess.Popen(
    ["timeout", "150", "tcpdump", "-i", BR, "-e", "-nn", "-c", "1", "-l",
     f"ether src {mac} and vlan 20"],
    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
time.sleep(1)
call("POST", f"/vms/{VM}/start")
ports = bridge_ports(BR)
check(len(ports) == 1 and ports[0].startswith("tap"), f"VM tap {ports} is a port of {BR}")
tap = ports[0]
vl = json.loads(sh(f"bridge -j vlan show dev {tap}"))
vlans = vl[0]["vlans"] if vl else []
check(any(v["vlan"] == 20 and "PVID" in v.get("flags", []) for v in vlans)
      and not any(v["vlan"] == 1 for v in vlans),
      f"tap {tap} has PVID 20 (VLAN tag) and not the default VLAN 1")
out, _ = cap.communicate()
check(mac in out.lower() and "vlan 20" in out,
      "guest PXE ROM sent a frame from the VM's MAC, tagged VLAN 20 on the bridge")
disk = vm["disks"][0]["path"] if vm.get("disks") else None
_, vm = call("GET", f"/vms/{VM}")
disk = vm["disks"][0]["path"]
qsock = os.path.splitext(disk)[0] + ".qmp.sock"
net = qmp(qsock, "human-monitor-command", {"command-line": "info network"})["return"]
check(mac in net and "user" not in net, "QEMU has exactly the configured tap NIC (no user-mode NIC)")

# --- Live CD-ROM insert / eject ------------------------------------------------
hw = vm["hardware"]
hw["cdrom"] = iso["path"]
call("PUT", f"/vms/{VM}/config", {"hardware": hw})
blocks = qmp(qsock, "query-block")["return"]
cd = next(b for b in blocks if b.get("qdev", "").endswith("cdrom0") or b["device"] == "cd0")
check(cd.get("inserted", {}).get("file") == iso["path"], "ISO inserted into the running VM")
hw["cdrom"] = None
call("PUT", f"/vms/{VM}/config", {"hardware": hw})
blocks = qmp(qsock, "query-block")["return"]
cd = next(b for b in blocks if b.get("qdev", "").endswith("cdrom0") or b["device"] == "cd0")
check("inserted" not in cd, "ISO ejected from the running VM")
hw2 = dict(hw, nets=[])
status, _ = call("PUT", f"/vms/{VM}/config", {"hardware": hw2}, ok=None)
check(status == 409, "removing a NIC while running is refused (409)")
status, _ = call("PUT", f"/vms/{VM}/config", {"memory": 256}, ok=None)
check(status == 409, "changing memory while running is refused (409)")

# --- Power actions -------------------------------------------------------------
status, body = call("POST", f"/vms/{VM}/reset", {})
check(status == 200, "Reset")
status, body = call("POST", f"/vms/{VM}/shutdown", {"timeout": 3}, ok=None)
check(status == 409 and "still running" in json.dumps(body),
      "Shutdown that the guest ignores reports it and leaves the VM running")
_, vm = call("GET", f"/vms/{VM}")
check(vm["status"] == "running", "VM still running after the ignored shutdown")
pid1 = open(f"/var/run/horcrux-vm-{VM}.pid").read().strip()
_, vm = call("POST", f"/vms/{VM}/reboot", {"timeout": 2})
pid2 = open(f"/var/run/horcrux-vm-{VM}.pid").read().strip()
check(vm["status"] == "running" and pid1 != pid2, "Reboot = shutdown + start (new QEMU)")
check(len(bridge_ports(BR)) == 1, "after reboot the VM is plugged into the bridge again")
_, vm = call("POST", f"/vms/{VM}/shutdown", {"timeout": 2, "force_stop": True})
check(vm["status"] == "stopped", "Shutdown with force stops the VM")
check(bridge_ports(BR) == [] and not os.path.exists(f"/sys/class/net/{tap}"),
      "tap device removed when the VM stops")

# --- VM firewall: a rule only on this VM's taps, enforced on bridged traffic --
FWBR, FWVM = "vmbr-fw", "nic-e2e-fw"
call("DELETE", f"/network/bridges/{FWBR}", ok=None)
call("POST", "/network/bridges", {"name": FWBR, "vlan_aware": False})
# A second port on the bridge: frames the guest sends are forwarded to it
# (through the bridge forward hook) and captured on its peer.
subprocess.run("ip link del fwa", shell=True, capture_output=True)
sh(f"ip link add fwa type veth peer name fwb && ip link set fwa master {FWBR} && ip link set fwa up && ip link set fwb up")
call("DELETE", f"/vms/{FWVM}", ok=None)
_, fwvm = call("POST", "/vms", {
    "id": FWVM, "name": FWVM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": {"nets": [{"model": "e1000", "bridge": FWBR}], "boot": ["net"]},
})
fwmac = fwvm["hardware"]["nets"][0]["mac"]
rule = {"id": "", "enabled": True, "action": "DROP", "direction": "out", "protocol": "udp",
        "source": None, "dest": None, "sport": None, "dport": "67", "comment": "no dhcp",
        "log": False, "position": 0}
_, created_rule = call("POST", f"/vms/{FWVM}/firewall/rules", rule)
_, listed = call("GET", f"/vms/{FWVM}/firewall/rules")
check(len(listed) == 1 and listed[0]["id"] == created_rule["id"], "VM firewall rule stored")
ruleset = sh("nft list table bridge horcrux")
check(f"jump vm-{FWVM}-out" in ruleset and "udp dport 67 drop" in ruleset and "iifname" in ruleset,
      "rule installed in the bridge family, jumped to from the VM's taps")


def capture_dhcp(seconds):
    """Start capturing the guest's DHCP on the other bridge port."""
    return subprocess.Popen(
        ["timeout", str(seconds), "tcpdump", "-i", "fwb", "-e", "-nn", "-c", "1", "-l",
         f"ether src {fwmac} and udp dst port 67"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)


def seen(cap):
    out, _ = cap.communicate()
    return fwmac in out.lower()


cap = capture_dhcp(45)
time.sleep(1)
call("POST", f"/vms/{FWVM}/start")
check(not seen(cap), "DROP udp/67 out: the guest's DHCP never leaves the bridge")
call("DELETE", f"/vms/{FWVM}/firewall/rules/{created_rule['id']}")
# Reset so the PXE ROM starts DHCP again (it gives up after a while).
cap = capture_dhcp(120)
time.sleep(1)
call("POST", f"/vms/{FWVM}/reset", {})
check(seen(cap), "rule deleted: the guest's DHCP reaches the other bridge port")
call("POST", f"/vms/{FWVM}/stop")
call("DELETE", f"/vms/{FWVM}")
sh("ip link del fwa")
call("DELETE", f"/network/bridges/{FWBR}")

# Cold edit applies at the next start.
_, vm = call("PUT", f"/vms/{VM}/config", {"memory": 192, "hardware": dict(hw, nets=[])})
check(vm["memory"] == 192 and vm["hardware"]["nets"] == [], "cold edit of memory + NICs")
status, body = call("DELETE", f"/network/bridges/{BR}", ok=None)
check(status == 204, "bridge deletable once no VM NIC uses it")
call("DELETE", f"/vms/{VM}")
print("VM networking e2e: all checks passed")
