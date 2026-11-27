#!/usr/bin/env python3
"""Run inside a privileged container by microseg_e2e.sh against the live API
with real nftables: guest microsegmentation must drop a denied flow, pass an
allowed one, move the per-rule counters and install the macvtap-style
netdev filters. Network layout (all inside the container):

  hxbr0 bridge  - port tap<hash>i0 (the VM's tap) <-> ns "guest" 10.77.0.10
                - port hxpeer1                    <-> ns "peer1" 10.77.0.20
                - port hxpeer2                    <-> ns "peer2" 10.77.0.30
  hxmv0 (netdev hooks) <-> ns "mv" 10.78.0.2, host side 10.78.0.1
"""
import json
import re
import ssl
import subprocess
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = ""


def call(method, path, body=None, ok=(200, 201, 204)):
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


def sh(cmd, check_rc=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check_rc and r.returncode != 0:
        raise SystemExit(f"FAIL `{cmd}`: {r.stderr.strip()}")
    return r


def fnv32(s):
    h = 0x811C9DC5
    for b in s.encode():
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def tap_prefix(vm_id):
    return f"tap{fnv32(vm_id):08x}i"


def ns(name, cmd, check_rc=True):
    return sh(f"ip netns exec {name} {cmd}", check_rc)


def can_connect(src_ns, ip, port, timeout=2):
    """TCP connect from a namespace; True when the handshake completes."""
    code = (
        "import socket,sys;s=socket.socket();s.settimeout(%d);"
        "r=s.connect_ex(('%s',%d));sys.exit(0 if r==0 else 1)" % (timeout, ip, port)
    )
    return ns(src_ns, f"python3 -c \"{code}\"", check_rc=False).returncode == 0


def serve(ns_name, port):
    sh(
        f"nohup ip netns exec {ns_name} python3 -m http.server {port} --bind 0.0.0.0 "
        f">/tmp/http-{ns_name}-{port}.log 2>&1 &"
    )


def rule(rid, **kw):
    r = {"id": rid, "enabled": True, "action": "DROP", "direction": "in", "protocol": None,
         "source": None, "dest": None, "sport": None, "dport": None, "comment": rid,
         "log": False, "position": 0}
    r.update(kw)
    return r


def hits(key):
    _, h = call("GET", f"/security/microseg/guests/{key}/rules")
    return {(x["direction"], x["rule_id"].split(" ")[0]): x for x in h}


def packets(key, direction, rid_prefix):
    return sum(
        x["packets"] for (d, r), x in hits(key).items() if d == direction and r.startswith(rid_prefix)
    )


def profile(**kw):
    p = {
        "enabled": True, "template": "custom", "default_in": "deny", "default_out": "deny",
        "allow_from": [], "allow_to": [], "lab_network": "", "ports": "",
        "macvtap_devices": [], "comment": "e2e",
    }
    p.update(kw)
    return p


def put_profile(key, **kw):
    _, a = call("PUT", f"/security/microseg/guests/{key}", profile(**kw))
    return a


# ------------------------------------------------------------------ setup
_, login = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
TOKEN = login["ticket"]

# a VM record (the profile is attached to an existing guest)
s, vm = call(
    "POST", "/vms",
    {"id": "mseg100", "name": "mseg-e2e", "hypervisor": "qemu", "memory": 256, "cpus": 1,
     "disk_size": 1, "status": "stopped", "architecture": "x86_64"},
    ok=None,
)
check(s in (200, 201), f"test VM created ({s} {str(vm)[:200]})")
VM = vm.get("id", "mseg100") if isinstance(vm, dict) else "mseg100"
KEY = f"vm:{VM}"
TAP = tap_prefix(VM) + "0"

sh("modprobe nf_conntrack_bridge nf_conntrack 2>/dev/null || true", check_rc=False)
sh("ip link add hxbr0 type bridge && ip link set hxbr0 up")
for name, tap, ip in (("guest", TAP, "10.77.0.10"), ("peer1", "hxpeer1", "10.77.0.20"),
                      ("peer2", "hxpeer2", "10.77.0.30")):
    sh(f"ip netns add {name}")
    sh(f"ip link add {tap} type veth peer name eth0 netns {name}")
    sh(f"ip link set {tap} master hxbr0 && ip link set {tap} up")
    ns(name, f"ip addr add {ip}/24 dev eth0")
    ns(name, "ip link set eth0 up")
    ns(name, "ip link set lo up")
time.sleep(2)  # bridge ports forwarding
for n, port in (("guest", 80), ("guest", 8080), ("guest", 5432), ("peer1", 80), ("peer1", 9999),
                ("peer2", 80), ("peer2", 9999)):
    serve(n, port)
time.sleep(1)

check(can_connect("peer2", "10.77.0.10", 8080), "before a profile, peer2 reaches the guest on 8080")
check(can_connect("guest", "10.77.0.30", 9999), "before a profile, the guest reaches peer2 on 9999")

# ------------------------------------------------------------ templates
_, t = call("GET", "/security/microseg/templates")
ids = {x["id"] for x in t}
check(ids >= {"custom", "web_server", "database", "isolated_lab", "pentest_box"}, "templates listed")

s, _ = call("PUT", f"/security/microseg/guests/{KEY}", profile(template="isolated_lab"), ok=None)
check(s == 400, "an isolated lab without a lab network is refused")
s, _ = call("PUT", f"/security/microseg/guests/{KEY}", profile(allow_from=[{"group": "+nope", "protocol": "any", "dport": "", "comment": ""}]), ok=None)
check(s == 400, "a group that does not exist is refused")
s, _ = call("PUT", "/security/microseg/guests/vm:nosuchvm", profile(), ok=None)
check(s == 404, "a profile for a guest that does not exist is refused")
s, _ = call("PUT", f"/security/microseg/guests/{KEY}", profile(template="pentest_box", lab_network="10.77.0.0/24", default_out="allow"), ok=None)
check(s == 400, "a pentest box cannot default to allow outbound")

# ------------------------------------------------------------ web server
a = put_profile(KEY, template="web_server")
check(a["warnings"] == [], f"web server profile installed without warnings: {a['warnings']}")
rs = sh("nft list ruleset").stdout
check(f"chain vm-{VM}-in" in rs and "hx:default" in rs, "guest chains with a default drop are in the kernel")
check("ct state established,related counter" in rs, "reply traffic of allowed flows is accepted first")

check(f'log prefix "hx-mseg: {KEY}/in "' in rs, "dropped flows are logged with the guest's key")
check(can_connect("peer2", "10.77.0.10", 80), "web server: allowed flow (tcp 80 in) passes")
check(not can_connect("peer2", "10.77.0.10", 8080), "web server: denied flow (tcp 8080 in) is dropped")
check(can_connect("guest", "10.77.0.30", 80), "web server: allowed outbound (tcp 80) passes")
check(not can_connect("guest", "10.77.0.30", 9999), "web server: denied outbound (tcp 9999) is dropped")

d_in = packets(KEY, "in", "default")
w_in = packets(KEY, "in", "mseg-web")
d_out = packets(KEY, "out", "default")
check(d_in > 0 and d_out > 0, f"default-deny counters moved (in {d_in}, out {d_out})")
check(w_in > 0, f"web rule counter moved ({w_in})")
check(can_connect("peer2", "10.77.0.10", 80), "one more allowed flow")
check(packets(KEY, "in", "mseg-web") > w_in, "allowed flows keep counting")
check(not can_connect("peer2", "10.77.0.10", 8080), "one more denied flow")
check(packets(KEY, "in", "default") > d_in, "dropped flows keep counting")

_, rows = call("GET", "/security/microseg/guests")
row = [r for r in rows if r["key"] == KEY][0]
check(row["enforced"] and row["template"] == "web_server" and row["dropped_packets"] > 0,
      f"guest list shows the enforced profile and its drops: {row}")

_, fv = call("GET", f"/security/microseg/guests/{KEY}/flows?ip=10.77.0.10")
check(any(f["dport"] == "80" and f["src"] in ("10.77.0.30", "10.77.0.10") for f in fv["flows"]),
      f"flow view lists the allowed flow: {len(fv['flows'])} flows, notes {fv['notes']}")

# ------------------------------------------------------------ database + group
call("POST", "/firewall/ipsets", {"name": "app", "comment": "e2e", "entries": ["10.77.0.20"]})
put_profile(KEY, template="database", allow_from=[{"group": "+app", "protocol": "any", "dport": "", "comment": "app servers"}],
            ports="5432")
check(can_connect("peer1", "10.77.0.10", 5432), "database: a member of the allowed group connects")
check(not can_connect("peer2", "10.77.0.10", 5432), "database: a host outside the group is dropped")
check(not can_connect("peer1", "10.77.0.10", 8080), "database: the group gets only the database port")
call("POST", "/firewall/ipsets/app/entries", {"cidr": "10.77.0.30"})
time.sleep(1)
check(can_connect("peer2", "10.77.0.10", 5432), "adding a host to the group opens the flow without another apply")
call("DELETE", "/firewall/ipsets/app/entries/10.77.0.30")
time.sleep(1)
check(not can_connect("peer2", "10.77.0.10", 5432), "removing the host closes it again")

# ------------------------------------------------------------ isolated lab / pentest
put_profile(KEY, template="isolated_lab", lab_network="10.77.0.20/32")
check(can_connect("guest", "10.77.0.20", 9999), "isolated lab: the lab host is reachable")
check(not can_connect("guest", "10.77.0.30", 9999), "isolated lab: anything else is dropped")
check(can_connect("peer1", "10.77.0.10", 8080), "isolated lab: the lab host may connect in")
check(not can_connect("peer2", "10.77.0.10", 8080), "isolated lab: others may not connect in")

put_profile(KEY, template="pentest_box", lab_network="+app", allow_from=[{"group": "10.77.0.30", "protocol": "tcp", "dport": "8080", "comment": "management"}])
check(can_connect("guest", "10.77.0.20", 9999), "pentest box: outbound to the lab works")
check(not can_connect("guest", "10.77.0.30", 80), "pentest box: outbound anywhere else is dropped")
check(can_connect("peer2", "10.77.0.10", 8080), "pentest box: the management group may connect in")
check(not can_connect("peer2", "10.77.0.10", 5432), "pentest box: the management group gets only its port")

# ------------------------------------------------------------ stored rules still apply
put_profile(KEY, template="custom", default_in="allow", default_out="allow")
check(can_connect("peer2", "10.77.0.10", 8080) and can_connect("guest", "10.77.0.30", 9999),
      "default allow both ways = nothing dropped")
s, _ = call("POST", f"/vms/{VM}/firewall/rules",
            rule("blk8080", source="10.77.0.30", dport="8080", protocol="tcp", direction="in"), ok=None)
check(s in (200, 201), f"stored guest rule added ({s})")
check(not can_connect("peer2", "10.77.0.10", 8080), "a stored guest rule still drops with a default-allow profile")
check(packets(KEY, "in", "blk8080") > 0, "and its hit counter moves")
call("DELETE", f"/vms/{VM}/firewall/rules/blk8080", ok=None)
check(can_connect("peer2", "10.77.0.10", 8080), "removing it opens the flow again")

# ------------------------------------------------------------ macvtap-style device filters
sh("ip netns add mv")
sh("ip link add hxmv0 type veth peer name eth0 netns mv")
sh("ip addr add 10.78.0.1/24 dev hxmv0 && ip link set hxmv0 up")
ns("mv", "ip addr add 10.78.0.2/24 dev eth0")
ns("mv", "ip link set eth0 up")
serve("mv", 7000)
sh("nohup python3 -m http.server 80 --bind 10.78.0.1 >/tmp/http-host-80.log 2>&1 &")
sh("nohup python3 -m http.server 9998 --bind 10.78.0.1 >/tmp/http-host-9998.log 2>&1 &")
time.sleep(1)
check(can_connect("mv", "10.78.0.1", 9998), "macvtap path: before a profile everything passes")
a = put_profile(KEY, template="web_server", macvtap_devices=["hxmv0"])
nft_ok = [w for w in a["warnings"] if "egress" in w or "hook" in w]
check(not a["warnings"] or nft_ok, f"netdev filters installed (warnings: {a['warnings']})")
rs = sh("nft list ruleset").stdout
if "table netdev hxmseg" in rs:
    check("hook ingress device \"hxmv0\"" in rs, "netdev ingress hook on the device (traffic from the guest)")
    check(can_connect("mv", "10.78.0.1", 80), "macvtap path: allowed outbound (tcp 80) passes")
    check(not can_connect("mv", "10.78.0.1", 9998), "macvtap path: denied outbound is dropped")
    check(packets(KEY, "out", "default") > 0, "the device chain's default counter moves")
else:
    print("note: netdev family not usable here:", a["warnings"], flush=True)

# ------------------------------------------------------------ switch off / remove
call("PUT", "/security/settings", {**call("GET", "/security/settings")[1], "microseg": {**call("GET", "/security/settings")[1]["microseg"], "enabled": False}})
check(can_connect("peer2", "10.77.0.10", 8080), "feature off: the kernel enforces nothing")
rs = sh("nft list ruleset").stdout
check("hx:default" not in rs, "feature off: no default-deny rule left in the kernel")
call("PUT", "/security/settings", {**call("GET", "/security/settings")[1], "microseg": {**call("GET", "/security/settings")[1]["microseg"], "enabled": True}})
check(not can_connect("peer2", "10.77.0.10", 8080), "feature back on: enforced again")

call("DELETE", f"/security/microseg/guests/{KEY}")
check(can_connect("peer2", "10.77.0.10", 8080) and can_connect("guest", "10.77.0.30", 9999),
      "profile removed: the guest is unrestricted again")
s, _ = call("GET", f"/security/microseg/guests/{KEY}", ok=None)
check(s == 404, "removed profile is gone")
call("DELETE", "/firewall/ipsets/app", ok=None)
print("microsegmentation e2e: ALL OK")
