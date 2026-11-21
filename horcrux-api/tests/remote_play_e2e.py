#!/usr/bin/env python3
"""Driven by remote_play_e2e.sh inside a privileged container, API on :8006.

Everything goes through the HTTP API (what the UI calls); results are
checked on the wire from separate network namespaces.
"""
import json
import socket
import ssl
import struct
import subprocess
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
BR = "rpnat0"
VM = "rp-e2e-vm"
LAN = "192.168.77.0/24"


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


def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"FAIL command: {cmd}\n{r.stdout}{r.stderr}")
    return r.stdout


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def wait_for(fn, secs, what):
    end = time.time() + secs
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(1)
    raise SystemExit("FAIL (timeout) " + what)


def in_ns(ns, py):
    """Run a python snippet in a network namespace, return its stdout."""
    return subprocess.run(["ip", "netns", "exec", ns, "python3", "-c", py],
                          capture_output=True, text=True, timeout=60).stdout


def varint(v):
    out = b""
    while True:
        b = v & 0x7F
        v >>= 7
        if v == 0:
            return out + bytes([b])
        out += bytes([b | 0x80])


def steam_packet(client_id, msg_type, seq):
    header = b"\x08" + varint(client_id) + b"\x10" + varint(msg_type)
    body = b"\x08" + varint(seq)
    magic = b"\xff\xff\xff\xff\x21\x4c\x5f\xa0"
    return magic + struct.pack("<I", len(header)) + header + struct.pack("<I", len(body)) + body


def vm_status():
    _, v = call("GET", f"/vms/{VM}")
    return v["status"]


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
TOKEN = r.get("ticket") or r.get("token")

# --- networks -------------------------------------------------------------
sh("sysctl -qw net.ipv4.ip_forward=1")
for ns, host_ip, peer_ip in (("lan", "192.168.77.1/24", "192.168.77.2/24"),
                             ("other", "10.9.9.1/24", "10.9.9.2/24")):
    sh(f"ip netns add {ns}")
    sh(f"ip link add h-{ns} type veth peer name p-{ns}")
    sh(f"ip link set p-{ns} netns {ns}")
    sh(f"ip addr add {host_ip} dev h-{ns} && ip link set h-{ns} up")
    sh(f"ip netns exec {ns} sh -c 'ip addr add {peer_ip} dev p-{ns} && ip link set p-{ns} up && ip link set lo up'")
    gw = host_ip.split("/")[0]
    sh(f"ip netns exec {ns} ip route add default via {gw}")

# NAT bridge for the VM (no physical port: Remote Play must use relay + forwards)
call("DELETE", f"/network/bridges/{BR}", ok=None)
call("POST", "/network/bridges", {"name": BR, "address": "10.50.0.1/24", "ports": []})

# --- the VM, Remote Play on (Steam + Moonlight) ------------------------------
call("POST", f"/vms/{VM}/stop", ok=None)
call("DELETE", f"/vms/{VM}", ok=None)
_, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": {"nets": [{"model": "virtio", "bridge": BR}], "boot": ["disk"]},
})
mac = vm["hardware"]["nets"][0]["mac"]
_, cfg = call("PUT", f"/vms/{VM}/remote-play", {
    "enabled": True, "clients": ["steam", "moonlight"], "network": "auto",
    "lan_sources": [LAN], "wake_on_lan": True, "wake_on_discovery": False,
})
check(cfg["enabled"] and cfg["lan_sources"] == [LAN], "Remote Play settings stored")
status, body = call("PUT", f"/vms/{VM}/remote-play",
                    {"enabled": True, "clients": ["steam"], "lan_sources": ["not-a-net"]}, ok=None)
check(status == 400, "an invalid allowed network is refused")
_, st = call("GET", f"/vms/{VM}/remote-play/status")
check(st["network"] == "nat", f"a card on a bridge without a LAN port is detected as NAT ({st['network']})")


def wol(ns, dst):
    pkt = "b'\\xff'*6 + bytes.fromhex('" + mac.replace(":", "") + "')*16"
    in_ns(ns, "import socket;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);"
              "s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1);"
              f"s.sendto({pkt},('{dst}',9))")


wait_for(lambda: ":9 " in sh("ss -lun") or "0.0.0.0:9 " in sh("ss -lun"), 20,
         "Wake-on-LAN listener on UDP 9")
check(True, "Wake-on-LAN listener on UDP 9")
wol("other", "10.9.9.255")
time.sleep(6)
check(vm_status() == "stopped", "a magic packet from a network that is not allowed does not start the VM")
wol("lan", "192.168.77.255")
wait_for(lambda: vm_status() == "running", 60, "VM started by Wake-on-LAN")
check(True, "a magic packet for the VM's MAC from the allowed network starts it")
_, ev = call("GET", "/audit/events?limit=50", ok=None)
check("remote_play_wake" in json.dumps(ev), "the wake is in the audit log")

# --- stand-in for the guest: a namespace with the VM's MAC on the NAT bridge --
ports = [l["ifname"] for l in json.loads(sh(f"ip -j link show master {BR}") or "[]")]
tap = [p for p in ports if p.startswith("tap")]
check(len(tap) == 1, f"the VM's tap is a port of {BR}")
sh(f"ip link set {tap[0]} nomaster")  # the guest has no OS; its netns answers instead
sh("ip netns add vm")
sh("ip link add veth-rpvm type veth peer name g0")
sh("ip link set g0 netns vm")
sh(f"ip link set veth-rpvm master {BR} && ip link set veth-rpvm up")
sh(f"ip netns exec vm sh -c 'ip link set g0 address {mac} && ip addr add 10.50.0.10/24 dev g0 "
   "&& ip link set g0 up && ip link set lo up && ip route add default via 10.50.0.1'")
def pings():
    return subprocess.run("ping -c1 -W1 10.50.0.10", shell=True, capture_output=True).returncode == 0


try:
    wait_for(pings, 40, "host reaches the guest stand-in")
except SystemExit:
    print("VM MAC", mac)
    print(sh(f"ip addr show {BR}; ip route; bridge link; bridge fdb show br {BR}; ip netns exec vm ip addr; ip neigh", check=False))
    raise
# a fake Steam host: answers discovery requests with a status, and a TCP
# Remote Play port that says hello
guest = subprocess.Popen(["ip", "netns", "exec", "vm", "python3", "-c", r'''
import socket, threading, struct
status = bytes.fromhex("%s")
def udp():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", 27036))
    while True:
        d, a = s.recvfrom(2048)
        if d[:8] == b"\xff\xff\xff\xff\x21\x4c\x5f\xa0":
            s.sendto(status, a)
def tcp():
    t = socket.socket(); t.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    t.bind(("0.0.0.0", 27037)); t.listen(5)
    while True:
        c, a = t.accept(); c.sendall(b"hx-steam-host"); c.close()
threading.Thread(target=udp, daemon=True).start()
tcp()
''' % steam_packet(4242, 1, 7).hex()])

_, st = wait_for(lambda: (lambda r: r if r[1]["address"] == "10.50.0.10" else None)(
    call("GET", f"/vms/{VM}/remote-play/status")), 30, "VM address learned")
check(True, "the VM's address is learned from the neighbour table (no guest agent)")
wait_for(lambda: "10.50.0.10" in sh("nft list ruleset", check=False), 30, "port forwards installed")
rules = sh("nft list table inet horcrux_remote_play")
check("192.168.77.0/24" in rules and "dnat ip to 10.50.0.10" in rules,
      "nftables forwards scoped to the allowed network")
check("27036" not in rules.split("udp dport")[1].split("}")[0] if "udp dport" in rules else True,
      "the discovery port is left to the relay, not forwarded")

# --- discovery through the relay --------------------------------------------
req = steam_packet(9001, 0, 1).hex()
out = in_ns("lan", f'''
import socket, time
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
s.bind(("0.0.0.0", 27036)); s.settimeout(0.5)
want = bytes.fromhex("{steam_packet(4242, 1, 7).hex()}")
got = ""
for _ in range(5):
    s.sendto(bytes.fromhex("{req}"), ("192.168.77.255", 27036))
    end = time.time() + 3
    while time.time() < end and not got:
        try:
            d, a = s.recvfrom(2048)  # our own broadcast comes back too
        except socket.timeout:
            continue
        if d == want:
            got = a[0]
    if got:
        break
print(got)
''').strip()
check(out == "192.168.77.1",
      f"a Steam discovery request from the LAN reaches the VM and its answer comes back via the host ({out!r})")
out = in_ns("other", f'''
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
s.bind(("0.0.0.0", 27036)); s.settimeout(0.5)
s.sendto(bytes.fromhex("{req}"), ("10.9.9.255", 27036))
import time
end = time.time() + 3
res = "silent"
while time.time() < end:
    try:
        d, a = s.recvfrom(2048)
    except socket.timeout:
        continue
    if a[0] != "10.9.9.2":
        res = "answered"
print(res)
''').strip()
check(out == "silent", "discovery from a network that is not allowed gets no answer")

# --- forwarded Remote Play TCP port -----------------------------------------
probe = '''
import socket
try:
    c = socket.create_connection(("%s", 27037), timeout=3); print(c.recv(64).decode())
except Exception as e:
    print("refused:", type(e).__name__)
'''
out = in_ns("lan", probe % "192.168.77.1").strip()
check(out == "hx-steam-host", f"TCP 27037 on the host reaches the VM from the LAN ({out!r})")
out = in_ns("other", probe % "10.9.9.1").strip()
check(out != "hx-steam-host", f"TCP 27037 is not forwarded from other networks ({out!r})")

# --- checklist / rules ---------------------------------------------------------
_, st = call("GET", f"/vms/{VM}/remote-play/status")
checks = {c["id"]: c["status"] for c in st["checks"]}
print("checklist:", checks, flush=True)
check(checks.get("network") == "warn", "checklist: NAT works but says a LAN card is better")
for k in ("address", "discovery", "forwards", "wake_on_lan", "host_shutdown"):
    check(checks.get(k) == "ok", f"checklist '{k}' is ok ({checks.get(k)})")
check(checks.get("encode") == "warn", "checklist: no GPU = software encoding, said plainly")
kinds = {(r["kind"], r["client"].lower(), r["protocol"]) for r in st["rules"] if r["applied"]}
check(("forward", "steam", "tcp") in kinds and ("relay", "steam", "udp") in kinds
      and ("forward", "moonlight", "tcp") in kinds, f"rule list shows applied forwards and relay ({kinds})")
check("dnat" in st["nft"], "status carries the nftables text")

# --- node settings through the API + declarative export -----------------------
_, s = call("GET", "/vms/remote-play/settings")
check(s["wol_ports"] == [9, 7] and s["steam_discovery_port"] == 27036, "node defaults")
status, _ = call("PUT", "/vms/remote-play/settings", dict(s, wol_ports=[]), ok=None)
check(status == 400, "node settings are validated")

# --- off again: everything removed --------------------------------------------
call("PUT", f"/vms/{VM}/remote-play", {"enabled": False, "clients": ["steam"]})
wait_for(lambda: "horcrux_remote_play" not in sh("nft list tables", check=False), 30,
         "nftables table removed")
check(True, "turning Remote Play off removes the port forwards")
wait_for(lambda: ":27036 " not in sh("ss -lun") and "0.0.0.0:9 " not in sh("ss -lun"), 30,
         "listeners closed")
check(True, "turning it off closes the relay and Wake-on-LAN listeners")
guest.kill()
call("POST", f"/vms/{VM}/stop", ok=None)
print("ALL REMOTE PLAY CHECKS PASSED")
