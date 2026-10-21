#!/usr/bin/env python3
"""Driven by sdn_cni_e2e.sh inside a privileged sibling container: the real
CNI bridge/host-local plugins, real network namespaces, real nftables."""
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
TOKEN = None
NET = "e2e-sdn"
CONF = "/tmp/sdn/cni.d"
LEASES = f"/var/lib/cni/networks/{NET}"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=60) as r:
            text = r.read().decode()
            return r.status, (json.loads(text) if text else None)
    except urllib.error.HTTPError as e:
        text = e.read().decode()
        try:
            return e.code, json.loads(text)
        except ValueError:
            return e.code, text


def expect(code, method, path, body=None):
    got, resp = call(method, path, body)
    if got != code:
        sys.exit(f"FAIL {method} {path}: expected {code}, got {got}: {resp}")
    return resp


def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and r.returncode != 0:
        sys.exit(f"FAIL `{cmd}`: {r.stdout} {r.stderr}")
    return r


def reaches(src, ip):
    return sh(f"ip netns exec {src} ping -c 2 -W 1 {ip}", check=False).returncode == 0


def http_ok(src, ip, port):
    r = sh(f"ip netns exec {src} curl -s -m 3 http://{ip}:{port}/hello.txt", check=False)
    return r.returncode == 0 and "hello-sdn" in r.stdout


def step(msg):
    print(f"[sdn-e2e] {msg}", flush=True)


def login():
    global TOKEN
    TOKEN = None
    resp = expect(200, "POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN = resp.get("ticket") or resp.get("token")


def restart_api():
    pid = open("/tmp/sdn/api.pid").read().strip()
    sh(f"kill {pid}; for i in $(seq 1 30); do kill -0 {pid} 2>/dev/null || break; sleep 1; done")
    sh("bash /tmp/sdn/start-api.sh")
    login()


login()

step("network names can't escape the conf dir; bad subnets are refused")
for bad in [
    {"name": "../../../tmp/qa-sdn-escape", "plugin_type": "bridge", "subnet": "10.253.9.0/24"},
    {"name": "e2e-bad", "plugin_type": "bridge", "subnet": "not-a-cidr"},
    {"name": "e2e-bad", "plugin_type": "bridge", "subnet": "10.253.9.0/24", "gateway": "10.9.9.1"},
]:
    expect(400, "POST", "/cni/networks", bad)
if os.path.exists("/tmp/qa-sdn-escape.conflist"):
    sys.exit("FAIL: traversal name wrote /tmp/qa-sdn-escape.conflist")
expect(400, "DELETE", "/cni/networks/..%2F..%2Ftmp%2Fx")

step("create a bridge network")
expect(200, "POST", "/cni/networks", {"name": NET, "plugin_type": "bridge", "bridge": "e2ebr0",
                                      "subnet": "10.253.0.0/24", "gateway": "10.253.0.1"})
if not os.path.exists(f"{CONF}/{NET}.conflist"):
    sys.exit("FAIL: conflist not written")

step("attach two network namespaces")
for ns in ["e2e-a", "e2e-b", "e2e-c"]:
    sh(f"ip netns del {ns} 2>/dev/null; ip netns add {ns} && ip netns exec {ns} ip link set lo up")
a = expect(200, "POST", "/cni/attach", {"container_id": "e2e-a", "network_name": NET,
                                        "netns_path": "/run/netns/e2e-a",
                                        "labels": {"app": "client"}})
b = expect(200, "POST", "/cni/attach", {"container_id": "e2e-b", "network_name": NET,
                                        "netns_path": "/run/netns/e2e-b",
                                        "labels": {"app": "server"}})
ip_a, ip_b = a["ip_address"], b["ip_address"]
print(f"  a={ip_a} ({a['interface_name']}) b={ip_b}")
if a["interface_name"] != "net0":
    sys.exit(f"FAIL: default interface name {a['interface_name']}")
sh(f"ip netns exec e2e-b ip -4 addr show net0 | grep -q {ip_b}")
atts = expect(200, "GET", "/cni/attachments")
if {x["container_id"] for x in atts if x["network_name"] == NET} != {"e2e-a", "e2e-b"}:
    sys.exit(f"FAIL: attachments not recorded: {atts}")
leases = sorted(f for f in os.listdir(LEASES) if f[0].isdigit())
if leases != sorted([ip_a, ip_b]):
    sys.exit(f"FAIL: host-local leases {leases}")
expect(409, "POST", "/cni/attach", {"container_id": "e2e-a", "network_name": NET,
                                    "netns_path": "/run/netns/e2e-a"})
if not reaches("e2e-a", ip_b) or not reaches("e2e-b", ip_a):
    sys.exit("FAIL: attached namespaces can't reach each other")
os.makedirs("/tmp/sdn/www", exist_ok=True)
open("/tmp/sdn/www/hello.txt", "w").write("hello-sdn\n")
subprocess.Popen("cd /tmp/sdn/www && exec ip netns exec e2e-b python3 -m http.server 8080",
                 shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for _ in range(20):
    if http_ok("e2e-a", ip_b, 8080):
        break
    time.sleep(0.5)
else:
    sys.exit("FAIL: http a -> b:8080 before any policy")

step("delete is refused while containers are attached")
resp = expect(409, "DELETE", f"/cni/networks/{NET}")
if "e2e-a" not in json.dumps(resp):
    sys.exit(f"FAIL: 409 should name the attached containers: {resp}")

step("deny-ingress policy blocks traffic to the server")
pol = {"id": "e2e-deny", "name": "e2e deny ingress", "namespace": "default",
       "pod_selector": {"match_labels": {"app": "server"}, "match_expressions": []},
       "policy_types": ["Ingress"], "ingress": [], "egress": [], "enabled": True}
r = expect(200, "POST", "/network-policies", pol)
enf = r["enforcement"]
print(f"  enforcement: {enf}")
if not enf["applied"] or enf["endpoints"] != 1:
    sys.exit(f"FAIL: policy not enforced: {enf}")
sh("nft list table bridge horcrux_netpol")
if reaches("e2e-a", ip_b):
    sys.exit("FAIL: deny-ingress policy did not block a -> b")
if http_ok("e2e-a", ip_b, 8080):
    sys.exit("FAIL: deny-ingress policy did not block http a -> b")
if enf["stateful"] and not reaches("e2e-b", ip_a):
    sys.exit("FAIL: b -> a (replies are established traffic) should still work")

step("an allow rule lets one port through")
pol["ingress"] = [{"from": [{"PodSelector": {"match_labels": {"app": "client"}, "match_expressions": []}}],
                   "ports": [{"protocol": "TCP", "port": 8080, "end_port": None}]}]
r = expect(200, "PUT", "/network-policies/e2e-deny", pol)
if not r["enforcement"]["applied"] or r["enforcement"]["rules"] != 1:
    sys.exit(f"FAIL: update not enforced: {r}")
if not http_ok("e2e-a", ip_b, 8080):
    sys.exit("FAIL: allowed port 8080 is still blocked")
if reaches("e2e-a", ip_b):
    sys.exit("FAIL: ping should still be blocked (only tcp/8080 allowed)")
view = expect(200, "GET", "/network-policies/e2e-deny/nftables")
if not any("tcp dport 8080 return" in l for l in view):
    sys.exit(f"FAIL: /nftables does not show the enforced rule: {view}")

step("restart (kernel table gone, as after a reboot): everything comes back")
sh("nft delete table bridge horcrux_netpol")
os.remove(f"{CONF}/{NET}.conflist")
restart_api()
nets = [n["name"] for n in expect(200, "GET", "/cni/networks")]
if NET not in nets:
    sys.exit(f"FAIL: network lost on restart: {nets}")
if not os.path.exists(f"{CONF}/{NET}.conflist"):
    sys.exit("FAIL: conflist not rewritten at startup")
atts = expect(200, "GET", "/cni/attachments")
if {x["container_id"] for x in atts if x["network_name"] == NET} != {"e2e-a", "e2e-b"}:
    sys.exit(f"FAIL: attachments lost on restart: {atts}")
if [p["id"] for p in expect(200, "GET", "/network-policies")] != ["e2e-deny"]:
    sys.exit("FAIL: policy lost on restart")
enf = expect(200, "GET", "/network-policies/enforcement")
if not enf["applied"] or enf["endpoints"] != 1:
    sys.exit(f"FAIL: not re-enforced at startup: {enf}")
sh("nft list table bridge horcrux_netpol")
if reaches("e2e-a", ip_b) or not http_ok("e2e-a", ip_b, 8080):
    sys.exit("FAIL: policy not enforced after restart")
expect(409, "DELETE", f"/cni/networks/{NET}")

if os.path.exists("/usr/local/bin/api_contract"):
    step("UI contract test (SDN parts) against this server, with a real netns")
    env = dict(os.environ, HORCRUX_CONTRACT_API=API, HORCRUX_CONTRACT_SDN_NETNS="/run/netns/e2e-c")
    r = subprocess.run(["/usr/local/bin/api_contract", "sdn_", "--nocapture", "--test-threads=1"],
                       env=env, capture_output=True, text=True)
    print(r.stdout[-3000:], r.stderr[-3000:])
    if r.returncode != 0 or "2 passed" not in r.stdout:
        sys.exit("FAIL: UI contract test (sdn_)")

step("deleting the policy removes the table and lets traffic through")
r = expect(200, "DELETE", "/network-policies/e2e-deny")
if r["enforcement"]["endpoints"] != 0:
    sys.exit(f"FAIL: {r}")
if sh("nft list table bridge horcrux_netpol", check=False).returncode == 0:
    sys.exit("FAIL: table still present with no policy")
if not reaches("e2e-a", ip_b):
    sys.exit("FAIL: traffic still blocked after deleting the policy")

step("detach releases the leases; delete removes the conflist")
for ct in ["e2e-a", "e2e-b"]:
    expect(200, "POST", "/cni/detach", {"container_id": ct, "network_name": NET})
leases = [f for f in os.listdir(LEASES) if f[0].isdigit()]
if leases:
    sys.exit(f"FAIL: leases left behind after detach: {leases}")
if sh("ip netns exec e2e-a ip link show net0", check=False).returncode == 0:
    sys.exit("FAIL: interface still in the namespace after detach")
expect(404, "POST", "/cni/detach", {"container_id": "e2e-a", "network_name": NET})
expect(200, "DELETE", f"/cni/networks/{NET}")
if os.path.exists(f"{CONF}/{NET}.conflist"):
    sys.exit("FAIL: conflist left after delete")
print("[sdn-e2e] all SDN checks passed", flush=True)
