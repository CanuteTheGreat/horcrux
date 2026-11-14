#!/usr/bin/env python3
"""Driven by ovs_e2e.sh (phase `missing` or `real`): the real API next to a real Open vSwitch."""
import json
import ssl
import subprocess
import sys
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = ""


def call(method, path, body=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=60) as r:
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


def sh(*a):
    return subprocess.run(a, capture_output=True, text=True)


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

if sys.argv[1] == "missing":
    st, body = call("GET", "/sdn/ovs/bridges", ok=None)
    check(st == 424 and "ovs-missing" in json.dumps(body), f"no Open vSwitch -> structured 424 ovs-missing (HTTP {st})")
    sys.exit(0)

st, body = call("GET", "/sdn/ovs/bridges")
check(body == [], "a fresh Open vSwitch has no bridges")

for bad in ("--help", "-x", "has space", "a;b", "0123456789abcdef"):
    st, _ = call("POST", "/sdn/ovs/bridges", {"name": bad, "datapath_type": "system", "protocols": []}, ok=None)
    check(st == 422, f"bridge name {bad!r} is refused (HTTP {st})")

b = {"name": "ovsbr0", "datapath_type": "system", "fail_mode": "standalone", "protocols": ["OpenFlow13"]}
_, info = call("POST", "/sdn/ovs/bridges", b)
check(info["name"] == "ovsbr0", "bridge created through the API")
check(sh("ovs-vsctl", "br-exists", "ovsbr0").returncode == 0, "ovs-vsctl sees the bridge")
check(sh("ovs-vsctl", "get", "bridge", "ovsbr0", "protocols").stdout.strip() == "[OpenFlow13]", "OpenFlow13 protocol is set on the bridge")
check(sh("ovs-vsctl", "get", "bridge", "ovsbr0", "fail_mode").stdout.strip() == "standalone", "fail mode is set")
st, _ = call("POST", "/sdn/ovs/bridges", b, ok=None)
check(st == 409, f"creating the same bridge again is a conflict (HTTP {st})")
check("ovsbr0" in call("GET", "/sdn/ovs/bridges")[1], "bridge is listed")

_, info = call("POST", "/sdn/ovs/bridges/ovsbr0/ports", {"name": "int0", "port_type": "internal", "tag": 30, "trunks": [], "options": {}})
check("int0" in info["ports"], "internal port added")
check(sh("ovs-vsctl", "get", "port", "int0", "tag").stdout.strip() == "30", "VLAN tag 30 is set on the port")
check(sh("ip", "link", "show", "int0").returncode == 0, "the internal port is a real network device")
_, info = call("POST", "/sdn/ovs/bridges/ovsbr0/ports", {"name": "vx100", "port_type": "vxlan", "tag": None, "trunks": [], "options": {"remote_ip": "192.0.2.50", "key": "100"}})
check("vx100" in info["ports"], "VXLAN port added")
opts = sh("ovs-vsctl", "get", "interface", "vx100", "options").stdout
check("192.0.2.50" in opts and "100" in opts, "VXLAN remote_ip and key are on the interface")
st, _ = call("POST", "/sdn/ovs/bridges/ovsbr0/ports", {"name": "bad", "port_type": "internal", "tag": 5000, "trunks": [], "options": {}}, ok=None)
check(st == 422, f"VLAN id 5000 is refused (HTTP {st})")
st, _ = call("POST", "/sdn/ovs/bridges/nosuch/ports", {"name": "p1", "port_type": "internal", "tag": None, "trunks": [], "options": {}}, ok=None)
check(st == 404, f"port on an unknown bridge is 404 (HTTP {st})")

call("POST", "/sdn/ovs/bridges/ovsbr0/flows", {"table": 0, "priority": 100, "matches": {"dl_type": "0x0806"}, "actions": ["drop"]})
flows = sh("ovs-ofctl", "-O", "OpenFlow13", "dump-flows", "ovsbr0").stdout
check("priority=100,arp" in flows and "actions=drop" in flows, "the ARP drop rule is in the switch's flow table")
shown = call("GET", "/sdn/ovs/bridges/ovsbr0/flows")[1]
check(any("actions=drop" in l for l in shown), "the API lists the flow")
st, _ = call("POST", "/sdn/ovs/bridges/ovsbr0/flows", {"table": 0, "priority": 1, "matches": {}, "actions": []}, ok=None)
check(st == 422, f"a flow without actions is refused (HTTP {st})")
call("DELETE", "/sdn/ovs/bridges/ovsbr0/flows")
flows = sh("ovs-ofctl", "-O", "OpenFlow13", "dump-flows", "ovsbr0").stdout
check("actions=drop" not in flows, "deleting the flows removes the drop rule")

call("DELETE", "/sdn/ovs/bridges/ovsbr0/ports/vx100")
check("vx100" not in sh("ovs-vsctl", "list-ports", "ovsbr0").stdout.split(), "port removed")
st, _ = call("DELETE", "/sdn/ovs/bridges/ovsbr0/ports/vx100", ok=None)
check(st == 404, f"removing it again is 404 (HTTP {st})")
call("DELETE", "/sdn/ovs/bridges/ovsbr0")
check(sh("ovs-vsctl", "br-exists", "ovsbr0").returncode != 0, "bridge removed")
check(sh("ip", "link", "show", "int0").returncode != 0, "its internal port device is gone")
st, _ = call("GET", "/sdn/ovs/bridges/ovsbr0", ok=None)
check(st == 404, f"unknown bridge is 404 (HTTP {st})")
print("ALL OK")
