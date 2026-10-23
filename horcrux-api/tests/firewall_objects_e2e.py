#!/usr/bin/env python3
"""Run inside the container by firewall_nft_e2e.sh against the live API: IP
sets, aliases, macros and default policies must end up in the kernel's
nftables ruleset (`nft list ruleset`)."""
import json
import re
import ssl
import subprocess
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None


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


def ruleset():
    return subprocess.run(["nft", "list", "ruleset"], capture_output=True, text=True, check=True).stdout


def rule(rid, **kw):
    r = {"id": rid, "enabled": True, "action": "DROP", "direction": "in", "protocol": None,
         "source": None, "dest": None, "sport": None, "dport": None, "comment": rid,
         "log": False, "position": 0}
    r.update(kw)
    return r


def lines_with(text, *needles):
    return [l for l in text.splitlines() if all(n in l for n in needles)]


_, login = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
TOKEN = login["ticket"]

# macros exist
_, macros = call("GET", "/firewall/macros")
check({m["name"] for m in macros} >= {"SSH", "DNS", "HTTPS"}, "macro list")

# references are checked when the rule is added
s, _ = call("POST", "/firewall/rules", rule("bad1", source="+nope"), ok=None)
check(s in (400, 422), "rule with an unknown IP set is refused")
s, _ = call("POST", "/firewall/rules", rule("bad2", dport="macro:Nope", protocol="tcp"), ok=None)
check(s in (400, 422), "rule with an unknown macro is refused")

# IP set + macro rule -> one kernel rule with the expanded set and port
call("POST", "/firewall/ipsets", {"name": "office", "comment": "e2e", "entries": ["10.1.0.0/16", "192.168.1.5"]})
s, _ = call("POST", "/firewall/ipsets", {"name": "bad", "entries": ["10.0.0.0/8; drop"]}, ok=None)
check(s == 400, "IP set entry that is not an address is refused")
call("POST", "/firewall/rules", rule("set-ssh", source="+office", dport="macro:SSH"))
call("POST", "/firewall/datacenter/apply")
rs = ruleset()
hit = lines_with(rs, "10.1.0.0/16", "192.168.1.5", "tcp dport 22", "drop")
check(len(hit) == 1, f"IP set + SSH macro installed as one rule: {hit}")

# changing the set rebuilds the kernel rule without another apply
call("POST", "/firewall/ipsets/office/entries", {"cidr": "172.20.0.1"})
rs = ruleset()
check(len(lines_with(rs, "172.20.0.1", "tcp dport 22")) == 1, "added entry reaches the kernel")
call("DELETE", "/firewall/ipsets/office/entries/192.168.1.5")
rs = ruleset()
check(not lines_with(rs, "192.168.1.5"), "removed entry leaves the kernel")

# in use -> refused; empty set -> rule matches nothing, so it is not installed
s, _ = call("DELETE", "/firewall/ipsets/office", ok=None)
check(s == 400, "IP set used by a rule cannot be deleted")
call("POST", "/firewall/ipsets", {"name": "nobody", "entries": []})
call("POST", "/firewall/rules", rule("set-empty", source="+nobody", dport="macro:HTTP"))
call("POST", "/firewall/datacenter/apply")
check(not lines_with(ruleset(), "tcp dport 80"), "rule over an empty IP set installs nothing")

# alias + two-protocol macro
call("POST", "/firewall/aliases", {"name": "web1", "cidr": "10.9.9.9", "comment": "e2e"})
call("POST", "/firewall/rules", rule("alias-dns", dest="web1", dport="macro:DNS", action="ACCEPT"))
call("POST", "/firewall/datacenter/apply")
rs = ruleset()
check(len(lines_with(rs, "daddr 10.9.9.9", "udp dport 53", "accept")) == 1, "alias + DNS macro (udp)")
check(len(lines_with(rs, "daddr 10.9.9.9", "tcp dport 53", "accept")) == 1, "alias + DNS macro (tcp)")
call("PUT", "/firewall/aliases/web1", {"cidr": "10.9.9.10"})
check(len(lines_with(ruleset(), "daddr 10.9.9.10", "udp dport 53")) == 1, "alias change reaches the kernel")
s, _ = call("DELETE", "/firewall/aliases/web1", ok=None)
check(s == 400, "alias used by a rule cannot be deleted")

# default policies on the host chains
call("PUT", "/firewall/options", {"policy_in": "drop", "policy_out": "accept"})
rs = ruleset()
m = re.search(r"chain dc-in \{[^}]*\}", rs, re.S)
check(m and "policy drop" in m.group(0), "dc-in chain has policy drop")
check(m and "ct state established,related accept" in m.group(0) and "22, 8006" in m.group(0),
      "implicit accepts keep the host reachable")
s, _ = call("PUT", "/firewall/options", {"policy_in": "reject", "policy_out": "accept"}, ok=None)
check(s == 400, "bad policy refused")
call("PUT", "/firewall/options", {"policy_in": "accept", "policy_out": "accept"})
m = re.search(r"chain dc-in \{[^}]*\}", ruleset(), re.S)
check(m and "policy accept" in m.group(0), "policy back to accept")

# clean up
for rid in ("set-ssh", "set-empty", "alias-dns"):
    call("DELETE", f"/firewall/rules/{rid}")
call("DELETE", "/firewall/ipsets/office")
call("DELETE", "/firewall/ipsets/nobody")
call("DELETE", "/firewall/aliases/web1")
print("firewall objects e2e: ALL OK")
