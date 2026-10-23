#!/usr/bin/env python3
"""Driven by system_admin_e2e.sh inside a privileged sibling container with
a real OpenRC, chrony, apt and a dummy interface. Every check looks at the
host itself, not only at the API's answer."""
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


def call(method, path, body=None, raw=False):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=120) as r:
            text = r.read().decode()
            if raw:
                return r.status, text
            return r.status, (json.loads(text) if text.strip() else None)
    except urllib.error.HTTPError as e:
        text = e.read().decode()
        try:
            return e.code, json.loads(text)
        except ValueError:
            return e.code, text


def expect(code, method, path, body=None, raw=False):
    got, resp = call(method, path, body, raw)
    if got != code:
        sys.exit(f"FAIL {method} {path}: expected {code}, got {got}: {str(resp)[:400]}")
    return resp


def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and r.returncode != 0:
        sys.exit(f"FAIL `{cmd}`: {r.stdout} {r.stderr}")
    return r


def ok(cond, what):
    if not cond:
        sys.exit(f"FAIL: {what}")
    print(f"  ok: {what}", flush=True)


def step(msg):
    print(f"[sysadmin-e2e] {msg}", flush=True)


def wait_task(upid, secs=300):
    end = time.time() + secs
    while time.time() < end:
        t = expect(200, "GET", f"/tasks/{upid}")
        if t["status"] != "running":
            return t
        time.sleep(2)
    sys.exit(f"FAIL: task {upid} still running after {secs}s")


def task_log(upid):
    return "\n".join(expect(200, "GET", f"/tasks/{upid}/log")["lines"])


def login():
    global TOKEN
    TOKEN = None
    st, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
    assert st == 200, (st, r)
    TOKEN = r.get("ticket") or r.get("token")


def main():
    login()

    step("host name, time zone, locale")
    cfg = expect(200, "GET", "/system/config")
    ok(cfg["hostname"] == sh("hostname -s").stdout.strip(), "GET /system/config reports the real host name")
    new = {"hostname": "hx-sysadm", "domain": "lab.test", "timezone": "Europe/Berlin", "locale": "de_DE.UTF-8"}
    expect(200, "POST", "/system/config", new)
    ok(sh("hostname").stdout.strip() == "hx-sysadm", "the kernel host name changed")
    ok(open("/etc/hostname").read().strip() == "hx-sysadm", "/etc/hostname written")
    ok("hx-sysadm.lab.test hx-sysadm" in open("/etc/hosts").read(), "/etc/hosts maps the FQDN")
    ok(os.readlink("/etc/localtime").endswith("Europe/Berlin"), "/etc/localtime points at the zone")
    ok("LANG=de_DE.UTF-8" in open("/etc/locale.conf").read(), "/etc/locale.conf written")
    got = expect(200, "GET", "/system/config")
    ok((got["hostname"], got["domain"], got["timezone"]) == ("hx-sysadm", "lab.test", "Europe/Berlin"), "config reads back")
    expect(422, "POST", "/system/config", {**new, "timezone": "Mars/Base"})
    expect(422, "POST", "/system/config", {**new, "hostname": "bad host"})

    step("network interfaces")
    sh("ip link add hxdummy0 type dummy")
    ifs = {i["name"]: i for i in expect(200, "GET", "/system/network/interfaces")}
    ok("hxdummy0" in ifs and "lo" not in ifs, "interfaces list shows the dummy link, not lo")
    body = {"name": "hxdummy0", "method": "static", "address": "192.0.2.10", "netmask": "255.255.255.0",
            "gateway": None, "mtu": 1400, "auto": True, "bridge": None, "bridge_ports": None, "vlan_id": None}
    expect(200, "POST", "/system/network/interfaces/hxdummy0", body)
    out = sh("ip -4 addr show hxdummy0").stdout
    ok("192.0.2.10/24" in out and "mtu 1400" in sh("ip link show hxdummy0").stdout, "address and MTU really applied")
    ok("UP" in sh("ip link show hxdummy0").stdout.split("\n")[0], "link brought up")
    expect(422, "POST", "/system/network/interfaces/hxdummy0", {**body, "netmask": "255.0.255.0"})
    expect(422, "POST", "/system/network/interfaces/hxdummy0", {**body, "gateway": "10.9.9.1"})
    expect(200, "POST", "/system/network/interfaces/hxdummy0", {**body, "method": "manual", "address": None, "netmask": None})
    ok("192.0.2." not in sh("ip -4 addr show hxdummy0").stdout, "manual clears the address")
    expect(404, "POST", "/system/network/interfaces/nosuch0", body)
    dflt = sh("ip -4 route show default").stdout.split()
    if "dev" in dflt:
        up = dflt[dflt.index("dev") + 1]
        cur = {i["name"]: i for i in expect(200, "GET", "/system/network/interfaces")}[up]
        expect(409, "POST", f"/system/network/interfaces/{up}", {**cur, "method": "static", "address": "198.51.100.7", "netmask": "255.255.255.0", "gateway": None})
        ok(True, "re-addressing the interface with the default route is refused (409)")

    step("DNS")
    orig_resolv = open("/etc/resolv.conf").read()
    expect(200, "POST", "/system/network/dns", {"servers": ["192.0.2.53", "2001:db8::53"], "search_domains": ["lab.test"]})
    rc = open("/etc/resolv.conf").read()
    ok("nameserver 192.0.2.53" in rc and "nameserver 2001:db8::53" in rc and "search lab.test" in rc, "/etc/resolv.conf rewritten")
    d = expect(200, "GET", "/system/network/dns")
    ok(d["servers"] == ["192.0.2.53", "2001:db8::53"] and d["search_domains"] == ["lab.test"], "DNS reads back")
    expect(422, "POST", "/system/network/dns", {"servers": ["not-an-ip"], "search_domains": []})
    expect(422, "POST", "/system/network/dns", {"servers": [], "search_domains": []})
    # put the real resolver back: the package steps below need the network
    real = [l.split()[1] for l in orig_resolv.splitlines() if l.startswith("nameserver")][:3]
    expect(200, "POST", "/system/network/dns", {"servers": real, "search_domains": []})
    ok(all(f"nameserver {n}" in open("/etc/resolv.conf").read() for n in real), "the original nameservers restored through the API")

    step("NTP (chrony)")
    expect(200, "POST", "/system/time/ntp", {"servers": ["ntp1.lab.test", "192.0.2.123"], "timezone": "UTC"})
    conf = open("/etc/chrony/chrony.conf").read()
    ok("server ntp1.lab.test iburst" in conf and "server 192.0.2.123 iburst" in conf and not any(l.startswith("pool ") for l in conf.splitlines()), "chrony.conf lists the new servers")
    n = expect(200, "GET", "/system/time/ntp")
    ok(n["servers"] == ["ntp1.lab.test", "192.0.2.123"] and n["timezone"] == "UTC", "NTP reads back")
    ok(os.readlink("/etc/localtime").endswith("/UTC"), "time zone changed together with NTP")
    expect(422, "POST", "/system/time/ntp", {"servers": ["bad host!"], "timezone": "UTC"})

    step("health")
    h = expect(200, "GET", "/system/health")
    ok(h["hostname"] == "hx-sysadm" and h["memory_usage"]["total"] > 0 and len(h["load_average"]) == 3, "health reads /proc")
    ok(h["system_info"]["cpu_cores"] >= 1 and h["system_info"]["kernel_version"] and h["disk_usage"], "kernel, CPU and disks reported")

    step("services (real OpenRC)")
    svc = {s["name"]: s for s in expect(200, "GET", "/system/services")}
    ok("hxtest" in svc and svc["hxtest"]["status"] == "inactive", "hxtest listed as stopped")
    ok(svc["hxtest"]["description"] == "Horcrux system-admin e2e service", "description read from the init script")
    expect(200, "POST", "/system/services/hxtest/start")
    svc = {s["name"]: s for s in expect(200, "GET", "/system/services")}
    pid = svc["hxtest"]["pid"]
    ok(svc["hxtest"]["status"] == "active" and pid and os.path.exists(f"/proc/{pid}"), "start: active with a live pid")
    ok("started" in sh("rc-service hxtest status", check=False).stdout, "rc-service agrees")
    expect(200, "POST", "/system/services/hxtest/enable")
    ok("hxtest" in sh("rc-update show").stdout, "enable: in a runlevel")
    expect(200, "POST", "/system/services/hxtest/restart")
    svc = {s["name"]: s for s in expect(200, "GET", "/system/services")}
    ok(svc["hxtest"]["status"] == "active" and svc["hxtest"]["enabled"] and svc["hxtest"]["pid"] != pid, "restart: new pid, enabled")
    expect(200, "POST", "/system/services/hxtest/disable")
    expect(200, "POST", "/system/services/hxtest/stop")
    svc = {s["name"]: s for s in expect(200, "GET", "/system/services")}
    ok(svc["hxtest"]["status"] == "inactive" and not svc["hxtest"]["enabled"], "stop + disable")
    expect(404, "POST", "/system/services/nosuchsvc/start")
    expect(400, "POST", "/system/services/hxtest/explode")
    expect(400, "POST", "/system/services/a;b/start")

    step("system log")
    with open("/var/log/messages", "w") as f:
        f.write("Oct  2 07:01:09 gw sshd[411]: Failed password for root\n")
        f.write("Oct  2 07:01:10 gw cron[12]: job started\n")
        f.write("Oct  2 07:01:11 gw kernel: warning: low memory\n")
    logs = expect(200, "GET", "/system/logs")
    ok(len(logs) == 3 and logs[0]["service"] == "kernel", "newest first from /var/log/messages")
    ok([e["service"] for e in expect(200, "GET", "/system/logs?level=warn")] == ["kernel", "sshd"], "level filter = warn and worse")
    ok(len(expect(200, "GET", "/system/logs?service=cron")) == 1, "service filter")
    ok(len(expect(200, "GET", "/system/logs?search=PASSWORD")) == 1, "search filter")
    ok(len(expect(200, "GET", "/system/logs?limit=1")) == 1, "limit")
    csv = expect(200, "GET", "/system/logs/export?format=csv", raw=True)
    ok(csv.startswith("timestamp,level,service,message") and "Failed password" in csv, "CSV export")
    expect(200, "DELETE", "/system/logs")
    ok(os.path.getsize("/var/log/messages") == 0 and expect(200, "GET", "/system/logs") == [], "clear truncated the log file")

    step("packages (apt)")
    pk = {p["name"]: p for p in expect(200, "GET", "/system/packages")}
    ok("bash" in pk and pk["bash"]["status"] in ("installed", "upgradeable") and pk["bash"]["installed_version"], "installed packages from dpkg")
    expect(400, "GET", "/system/packages/search?q=a")
    ok(any(p["name"] == "hello" for p in expect(200, "GET", "/system/packages/search?q=hello")), "repository search")
    up = expect(200, "POST", "/system/packages/update")
    t = wait_task(up["upid"])
    ok(t["status"] == "ok" and t["task_type"] == "package-sync", "apt-get update ran as a task")
    ins = expect(200, "POST", "/system/packages/hello/install")
    t = wait_task(ins["upid"])
    ok(t["status"] == "ok" and sh("dpkg -s hello", check=False).returncode == 0, "install: dpkg has the package")
    ok("hello" in task_log(ins["upid"]) and "TASK OK" in task_log(ins["upid"]), "the task log holds apt's output")
    pk = {p["name"]: p for p in expect(200, "GET", "/system/packages")}
    ok(pk["hello"]["status"] in ("installed", "upgradeable"), "listed as installed")
    rm = expect(200, "POST", "/system/packages/hello/remove")
    t = wait_task(rm["upid"])
    ok(t["status"] == "ok" and sh("dpkg -s hello 2>&1 | grep -q 'Status: install ok installed'", check=False).returncode != 0, "remove: gone from dpkg")
    expect(404, "POST", "/system/packages/hello/remove")
    expect(422, "POST", "/system/packages/-y/install")
    expect(422, "POST", "/system/packages/a;b/install")
    # one package task at a time
    first = expect(200, "POST", "/system/packages/update")
    got, _ = call("POST", "/system/packages/update")
    ok(got == 409, "a second package task while one runs is refused (409)")
    wait_task(first["upid"])

    step("task log")
    tl = expect(200, "GET", "/tasks?limit=50")
    types = {t["task_type"] for t in tl}
    ok({"package-install", "package-remove", "package-sync", "service", "system-config", "dns-config", "ntp-config", "log-clear", "net-config"} <= types,
       f"every operation above is in the task log ({sorted(types)})")
    ok(tl == sorted(tl, key=lambda t: -t["start_time"]), "newest first")
    ok(all(t["status"] == "ok" for t in expect(200, "GET", "/tasks?status=ok")), "status filter")
    expect(404, "GET", "/tasks/UPID:nosuch:0:x:y:z:")
    expect(400, "GET", "/tasks/not-a-upid")
    expect(409, "DELETE", f"/tasks/{ins['upid']}")  # finished: nothing to stop

    step("diagnostics")
    d = expect(200, "GET", "/system/diagnostics")
    ok(len(d["available_tests"]) >= 10 and 0 <= d["overall_health_score"] <= 100, "overview with scores")
    r = expect(200, "POST", "/system/diagnostics/tests/data-dir-writable")
    ok(r["passed"], "data dir writable test passes")
    ok(expect(200, "POST", "/system/diagnostics/tests/database")["passed"], "database test passes")
    expect(404, "POST", "/system/diagnostics/tests/nosuch")
    allr = expect(200, "POST", "/system/diagnostics/tests/run-all")
    ok(set(allr) >= {"disk-space", "memory", "kvm", "qemu", "database"}, "run-all returns every test")
    rep = expect(200, "GET", "/system/diagnostics/report?format=markdown", raw=True)
    ok(rep.startswith("# Diagnostics for hx-sysadm"), "markdown report")

    step("SDN zones, VNets, subnets, IPAM (real bridges, VLAN/VXLAN devices, nftables)")
    expect(200, "POST", "/sdn/zones", {"id": "lab", "zone_type": "simple"})
    expect(409, "POST", "/sdn/zones", {"id": "lab", "zone_type": "simple"})
    expect(422, "POST", "/sdn/zones", {"id": "bad1", "zone_type": "evpn"})
    expect(422, "POST", "/sdn/zones", {"id": "bad2", "zone_type": "vlan"})
    expect(422, "POST", "/sdn/zones", {"id": "bad3", "zone_type": "vxlan", "peers": ["not-an-ip"]})
    expect(200, "POST", "/sdn/vnets", {"id": "web", "zone": "lab", "vlan_aware": True})
    out = sh("ip -d link show web").stdout
    ok("bridge" in out and "vlan_filtering 1" in out and "UP" in out.split("\n")[0], "a simple VNet is a real, VLAN-aware, up bridge")
    expect(409, "POST", "/sdn/vnets", {"id": "web", "zone": "lab"})
    dflt_if = sh("ip -o -4 route show default").stdout.split()
    if "dev" in dflt_if:
        expect(409, "POST", "/sdn/vnets", {"id": dflt_if[dflt_if.index("dev") + 1][:8], "zone": "lab"})
        ok(True, "a VNet may not adopt an existing interface")
    expect(422, "POST", "/sdn/vnets", {"id": "Bad", "zone": "lab"})
    expect(422, "POST", "/sdn/vnets", {"id": "nozone", "zone": "missing"})

    expect(200, "POST", "/sdn/zones", {"id": "vl", "zone_type": "vlan", "uplink": "hxdummy0"})
    expect(200, "POST", "/sdn/vnets", {"id": "dmz", "zone": "vl", "tag": 30})
    out = sh("ip -d link show dmzv30").stdout
    ok("vlan" in out and "id 30" in out and "master dmz" in out, "a VLAN VNet gets a tagged sub-interface of the uplink, enslaved to its bridge")
    expect(409, "POST", "/sdn/vnets", {"id": "dmz2", "zone": "vl", "tag": 30})
    expect(422, "POST", "/sdn/vnets", {"id": "dmz3", "zone": "vl"})
    expect(200, "POST", "/sdn/zones", {"id": "badup", "zone_type": "vlan", "uplink": "nosuch9"})
    r = expect(500, "POST", "/sdn/vnets", {"id": "bad", "zone": "badup", "tag": 5})
    ok(call("GET", "/sdn/vnets")[1] and all(v["id"] != "bad" for v in expect(200, "GET", "/sdn/vnets")) and sh("ip link show bad", check=False).returncode != 0, "a failed apply leaves no VNet and no half-built bridge")
    expect(200, "DELETE", "/sdn/zones/badup")

    expect(200, "POST", "/sdn/zones", {"id": "ov", "zone_type": "vxlan", "peers": ["192.0.2.21", "192.0.2.22"], "mtu": 1450})
    expect(200, "POST", "/sdn/vnets", {"id": "tun", "zone": "ov", "tag": 4242})
    out = sh("ip -d link show vx4242").stdout
    ok("vxlan" in out and "id 4242" in out and "dstport 4789" in out and "master tun" in out, "a VXLAN VNet is a real vxlan device in the bridge")
    fdb = sh("bridge fdb show dev vx4242").stdout
    ok("dst 192.0.2.21" in fdb and "dst 192.0.2.22" in fdb, "head-end replication to both peers")
    z = expect(200, "GET", "/sdn/zones/ov")
    expect(200, "PUT", "/sdn/zones/ov", {**z, "peers": ["192.0.2.22", "192.0.2.23"]})
    fdb = sh("bridge fdb show dev vx4242").stdout
    ok("dst 192.0.2.23" in fdb and "dst 192.0.2.21" not in fdb, "changing the zone peers updates the kernel flood list")
    expect(409, "PUT", "/sdn/zones/ov", {**z, "zone_type": "simple"})

    expect(422, "POST", "/sdn/vnets/web/subnets", {"cidr": "10.77.0.5/24"})
    expect(422, "POST", "/sdn/vnets/web/subnets", {"cidr": "10.77.0.0/24", "snat": True})
    expect(422, "POST", "/sdn/vnets/web/subnets", {"cidr": "10.77.0.0/24", "gateway": "10.88.0.1"})
    sub = expect(200, "POST", "/sdn/vnets/web/subnets", {"cidr": "10.77.0.0/24", "gateway": "10.77.0.1", "snat": True})
    sid = sub["id"]
    ok(sid == "web-10.77.0.0-24" and "10.77.0.1/24" in sh("ip -4 addr show web").stdout, "the gateway address is on the bridge")
    nft = sh("nft list table inet horcrux_sdn").stdout
    ok("masquerade" in nft and "10.77.0.0/24" in nft and 'oifname != "web"' in nft, "SNAT is a real nftables masquerade rule")
    ok(open("/proc/sys/net/ipv4/ip_forward").read().strip() == "1", "IPv4 forwarding enabled")
    expect(422, "POST", "/sdn/vnets/web/subnets", {"cidr": "10.77.0.128/25"})
    expect(404, "POST", "/sdn/vnets/nosuch/subnets", {"cidr": "10.80.0.0/24"})
    st = {s["vnet"]: s for s in expect(200, "GET", "/sdn/status")}
    ok(st["web"]["bridge_present"] and "10.77.0.1/24" in st["web"]["addresses"], "status reports the bridge and its address")
    ok(st["dmz"]["carrier"] == "dmzv30" and st["dmz"]["carrier_present"] and "dmzv30" in st["dmz"]["ports"], "status reports the VLAN carrier as a bridge port")
    ok(st["tun"]["carrier"] == "vx4242" and st["tun"]["carrier_present"], "status reports the VXLAN carrier")

    a1 = expect(200, "POST", f"/sdn/subnets/{sid}/ipam", {"hostname": "vm1", "mac": "AA:BB:CC:00:00:01"})
    ok(a1["ip"] == "10.77.0.2" and a1["mac"] == "aa:bb:cc:00:00:01", "IPAM gives the lowest free address (not the gateway)")
    a2 = expect(200, "POST", f"/sdn/subnets/{sid}/ipam", {"hostname": "vm2", "ip": "10.77.0.50"})
    ok(a2["ip"] == "10.77.0.50", "IPAM honours a requested address")
    expect(409, "POST", f"/sdn/subnets/{sid}/ipam", {"hostname": "vm1"})
    expect(409, "POST", f"/sdn/subnets/{sid}/ipam", {"ip": "10.77.0.50"})
    expect(422, "POST", f"/sdn/subnets/{sid}/ipam", {"ip": "10.99.0.5"})
    expect(422, "POST", f"/sdn/subnets/{sid}/ipam", {"mac": "nope"})
    view = expect(200, "GET", f"/sdn/subnets/{sid}/ipam")
    ok(view["used"] == 3 and view["total"] == 254 and view["free"] == 251, "IPAM counts the gateway, vm1 and vm2")
    expect(409, "DELETE", f"/sdn/subnets/{sid}/ipam/10.77.0.1")
    expect(409, "DELETE", f"/sdn/vnets/web")
    expect(409, "DELETE", f"/sdn/subnets/{sid}")
    expect(409, "DELETE", "/sdn/zones/lab")

    # the kernel forgot (as after a reboot): apply recreates everything
    sh("ip link del web; ip link del vx4242; ip link del tun")
    res = expect(200, "POST", "/sdn/apply")
    ok(all(r["ok"] for r in res), f"apply reports success ({res})")
    ok("10.77.0.1/24" in sh("ip -4 addr show web").stdout and "id 4242" in sh("ip -d link show vx4242").stdout
       and "dst 192.0.2.23" in sh("bridge fdb show dev vx4242").stdout, "apply rebuilt bridges, address, VXLAN device and peers")
    # and a real API restart does the same by itself
    sh("ip link del web; ip link del tun; nft delete table inet horcrux_sdn")
    sh("kill $(cat /tmp/sa/api.pid); sleep 2; bash /tmp/sa/start-api.sh")
    login()
    end = time.time() + 40
    while time.time() < end and sh("ip link show web >/dev/null 2>&1 && ip link show tun >/dev/null 2>&1 && nft list table inet horcrux_sdn >/dev/null 2>&1", check=False).returncode != 0:
        time.sleep(2)
    ok("10.77.0.1/24" in sh("ip -4 addr show web").stdout and "masquerade" in sh("nft list table inet horcrux_sdn").stdout, "after an API restart the VNets, address and SNAT come back by themselves")
    ok(len(expect(200, "GET", f"/sdn/subnets/{sid}/ipam")["allocations"]) == 3, "IPAM allocations survived the restart")

    expect(200, "DELETE", f"/sdn/subnets/{sid}/ipam/10.77.0.2")
    expect(200, "DELETE", f"/sdn/subnets/{sid}/ipam/10.77.0.50")
    expect(404, "DELETE", f"/sdn/subnets/{sid}/ipam/10.77.0.50")
    expect(200, "DELETE", f"/sdn/subnets/{sid}")
    ok("10.77.0.1" not in sh("ip -4 addr show web").stdout and sh("nft list table inet horcrux_sdn", check=False).returncode != 0, "deleting the subnet removes the address and the SNAT table")
    for v in ("web", "dmz", "tun"):
        expect(200, "DELETE", f"/sdn/vnets/{v}")
    ok(all(sh(f"ip link show {l}", check=False).returncode != 0 for l in ("web", "dmz", "dmzv30", "tun", "vx4242")), "deleting the VNets removed their bridges and carriers")
    for z in ("lab", "vl", "ov"):
        expect(200, "DELETE", f"/sdn/zones/{z}")
    ok(expect(200, "GET", "/sdn/zones") == [], "all zones gone")

    step("audit log (search, filters, export, survives a restart)")
    pg = expect(200, "GET", "/audit/search?user=admin&event_type=Login&limit=50")
    ok(pg["total"] >= 2 and all(e["user"] == "admin" and e["event_type"] == "Login" for e in pg["events"]),
       f"login events are searchable ({pg['total']} found)")
    ok(pg["events"][0]["timestamp"] >= pg["events"][-1]["timestamp"], "newest first")
    ok(pg["total"] >= 2, "the login from before the API restart was reloaded from the audit log file")
    ok(expect(200, "GET", "/audit/search?search=zz-no-such-thing-zz")["total"] == 0, "free-text search")
    ok(len(expect(200, "GET", "/audit/search?limit=1")["events"]) == 1, "limit")
    fo = expect(200, "GET", "/audit/filter-options")
    ok("admin" in fo["users"] and "Login" in fo["event_types"], "filter options list what is in the log")
    csv = expect(200, "GET", "/audit/export?format=csv&user=admin", raw=True)
    ok(csv.startswith("timestamp,event_type,severity,user,") and '"admin"' in csv, "CSV export")
    js = json.loads(expect(200, "GET", "/audit/export?format=json&event_type=Login", raw=True))
    ok(js and js[0]["event_type"] == "Login", "JSON export")
    expect(400, "GET", "/audit/export?format=xml")
    call("POST", "/auth/login", {"username": "admin", "password": "wrong-password"})
    ok(len(expect(200, "GET", "/audit/failed-logins?limit=10")) >= 1 or expect(200, "GET", "/audit/search?event_type=LoginFailed")["total"] >= 1, "a failed login is recorded")

    step("node shell (a real PTY login shell over the WebSocket)")
    import base64, socket, struct
    tk = expect(200, "POST", "/nodes/shell/ticket")
    ok(tk["ws_path"].endswith(tk["ticket"]), "a ticket is minted")

    def ws_connect(path):
        raw = socket.create_connection(("127.0.0.1", 8006), timeout=20)
        sock = CTX.wrap_socket(raw, server_hostname="127.0.0.1")
        key = base64.b64encode(os.urandom(16)).decode()
        sock.sendall((f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:8006\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                      f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {TOKEN}\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            c = sock.recv(1)
            if not c:
                break
            head += c
        return sock, head.decode(errors="replace")

    def ws_send(sock, payload, opcode=1):
        data = payload.encode() if isinstance(payload, str) else payload
        mask = os.urandom(4)
        n = len(data)
        hdr = bytes([0x80 | opcode]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n))
        sock.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def ws_read(sock, until, secs=20):
        buf = b""
        end = time.time() + secs
        sock.settimeout(2)
        while time.time() < end and until not in buf.decode(errors="replace"):
            try:
                h = sock.recv(2)
            except socket.timeout:
                continue
            if len(h) < 2:
                break
            ln = h[1] & 0x7F
            if ln == 126:
                ln = struct.unpack(">H", sock.recv(2))[0]
            elif ln == 127:
                ln = struct.unpack(">Q", sock.recv(8))[0]
            data = b""
            while len(data) < ln:
                data += sock.recv(ln - len(data))
            if h[0] & 0x0F in (1, 2):
                buf += data
        return buf.decode(errors="replace")

    sock, head = ws_connect(tk["ws_path"])
    ok(" 101 " in head.split("\r\n")[0], f"the WebSocket upgrades ({head.splitlines()[0] if head else head})")
    ws_send(sock, '{"resize":[100,30]}')
    ws_send(sock, "echo hxshell-$((6*7)); tty; id -un; stty size\r")
    out = ws_read(sock, "30 100")
    ok("hxshell-42" in out, "commands run in the shell")
    ok("/dev/pts/" in out, "it has a real controlling PTY")
    ok("30 100" in out, "the window size from the resize message reached the PTY")
    ws_send(sock, "echo secret=${ADMIN_PASSWORD:-unset}\r")
    ok("secret=unset" in ws_read(sock, "secret=unset"), "the API's environment secrets are not passed to the shell")
    def shell_pids():
        out = []
        for d in os.listdir("/proc"):
            if d.isdigit():
                try:
                    argv = open(f"/proc/{d}/cmdline", "rb").read().split(b"\0")
                    if argv[0].endswith((b"bash", b"/sh")) and argv[1:2] == [b"-l"]:
                        out.append(d)
                except OSError:
                    pass
        return out

    pids = shell_pids()
    ok(len(pids) == 1, f"one login shell process exists ({pids})")
    pid = pids[0]
    sock.close()
    for _ in range(20):
        if sh(f"kill -0 {pid} 2>/dev/null", check=False).returncode != 0:
            break
        time.sleep(0.5)
    ok(sh(f"kill -0 {pid} 2>/dev/null", check=False).returncode != 0, "closing the browser socket ends the shell")
    sock2, head2 = ws_connect(tk["ws_path"])
    ok(" 404 " in head2.split("\r\n")[0], "a ticket works only once")
    sock2.close()
    for _ in range(20):
        tl = expect(200, "GET", "/tasks?type=node-shell")
        if tl:
            break
        time.sleep(0.5)
    ok(tl and tl[0]["user"] == "admin", "the session is in the task log")
    ok("shell session for admin" in task_log(tl[0]["upid"]), "with who and how long")

    step("packages (Portage branch; emerge itself is a recording shim)")
    # Real Portage is only in the Gentoo image job; here the vdb/repo layout
    # is real files and emerge is a shim that records its argv.
    os.makedirs("/var/db/pkg/app-editors/vim-9.0.1-r1", exist_ok=True)
    open("/var/db/pkg/app-editors/vim-9.0.1-r1/DESCRIPTION", "w").write("Vim, an improved vi\n")
    os.makedirs("/var/db/pkg/sys-apps/foo-1.9", exist_ok=True)
    os.makedirs("/var/db/repos/gentoo/app-editors/vim", exist_ok=True)
    open("/var/db/repos/gentoo/app-editors/vim/vim-9.0.1.ebuild", "w").write('DESCRIPTION="Vim text editor"\n')
    os.makedirs("/var/db/repos/gentoo/sys-apps/foo", exist_ok=True)
    os.makedirs("/var/db/repos/gentoo/app-misc/foo", exist_ok=True)
    open("/usr/local/bin/emerge", "w").write(
        "#!/bin/sh\necho \"emerge $*\" >> /tmp/emerge.log\necho \"shim ran: $*\"\n"
        "case \"$*\" in *--pretend*) echo '[ebuild     U  ] sys-apps/foo-2.0:0 [1.9]';; esac\nexit 0\n")
    os.chmod("/usr/local/bin/emerge", 0o755)
    pk = {p["name"]: p for p in expect(200, "GET", "/system/packages")}
    ok(pk["app-editors/vim"]["installed_version"] == "9.0.1-r1" and pk["app-editors/vim"]["description"] == "Vim, an improved vi", "installed packages come from /var/db/pkg")
    s = expect(200, "GET", "/system/packages/search?q=vim")
    ok(s and s[0]["name"] == "app-editors/vim" and s[0]["status"] == "installed" and s[0]["description"] == "Vim text editor", "tree search")
    t = wait_task(expect(200, "POST", "/system/packages/update")["upid"])
    ok(t["status"] == "ok" and "--sync" in open("/tmp/emerge.log").read(), "update runs emerge --sync")
    pk = {p["name"]: p for p in expect(200, "GET", "/system/packages")}
    ok(pk["sys-apps/foo"]["status"] == "upgradeable" and pk["sys-apps/foo"]["available_version"] == "2.0", "pending upgrades are read from emerge --pretend")
    ins = expect(200, "POST", "/system/packages/app-editors%2Fvim/install")
    wait_task(ins["upid"])
    ok("--noreplace app-editors/vim" in open("/tmp/emerge.log").read(), "install passes the atom to emerge")
    r = expect(422, "POST", "/system/packages/foo/install")
    ok("ambiguous" in json.dumps(r), "an ambiguous bare name is refused")
    rm = expect(200, "POST", "/system/packages/sys-apps%2Ffoo/remove")
    wait_task(rm["upid"])
    ok("--depclean sys-apps/foo" in open("/tmp/emerge.log").read(), "remove uses --depclean")
    expect(422, "POST", "/system/packages/..%2F..%2Fetc/install")

    step("Ceph management (the `ceph` CLI is a shim that serves recorded Ceph JSON and logs its arguments; no live cluster in CI)")
    st = expect(200, "GET", "/ceph/status")
    ok(not st["available"] and st["health"] == "NOT_INSTALLED", "without Ceph installed the status says so")
    expect(503, "GET", "/ceph/osds")
    open("/usr/local/bin/ceph", "w").write("""#!/bin/sh
echo "ceph $*" >> /tmp/ceph.log
fx=/tmp/ceph-fx
case "$*" in
 "status "*) cat $fx/status.json;;
 "mon dump "*) cat $fx/mon_dump.json;;
 "osd tree "*) cat $fx/osd_tree.json;;
 "osd pool ls detail "*) cat $fx/pools.json;;
 "df "*) cat $fx/df.json;;
 "osd crush rule dump "*) cat $fx/crush_rules.json;;
 "osd dump "*) cat $fx/osd_dump.json;;
 "osd out "*|"osd in "*|"osd set "*|"osd unset "*|"osd pool create "*|"osd pool set "*|"osd pool application enable "*|"osd pool delete "*) echo done;;
 *) echo "unknown: $*" >&2; exit 22;;
esac
""")
    os.chmod("/usr/local/bin/ceph", 0o755)
    cl = lambda: open("/tmp/ceph.log").read()
    st = expect(200, "GET", "/ceph/status")
    ok(st["reachable"] and st["health"] == "HEALTH_WARN" and st["osds_up"] == 2 and st["osds_total"] == 3 and st["mons_in_quorum"] == ["a", "b", "c"],
       "status parses the cluster's JSON")
    ok([h["name"] for h in st["health_checks"]] == ["OSD_DOWN", "PG_DEGRADED"], "health checks listed")
    osds = expect(200, "GET", "/ceph/osds")
    ok([(o["name"], o["host"], o["status"], o["is_in"]) for o in osds] == [("osd.0", "pve1", "up", True), ("osd.1", "pve1", "down", False), ("osd.2", "pve2", "up", True)], "OSDs with host and state")
    ok(len(expect(200, "GET", "/ceph/monitors")) == 3, "monitors")
    pl = expect(200, "GET", "/ceph/pools")
    ok([p["name"] for p in pl] == [".mgr", "vmdata"] and pl[1]["stored_bytes"] == 134217728 and pl[1]["applications"] == ["rbd"], "pools joined with usage")
    ok(len(expect(200, "GET", "/ceph/crush-rules")) == 2, "CRUSH rules")
    fl = {f["name"]: f["set"] for f in expect(200, "GET", "/ceph/flags")}
    ok(fl["noout"] and not fl["norebalance"], "flags from the OSD map")

    expect(200, "POST", "/ceph/osds/1/in")
    ok("osd in 1" in cl(), "OSD in runs `ceph osd in 1`")
    expect(200, "POST", "/ceph/osds/2/out")
    expect(404, "POST", "/ceph/osds/9/out")
    expect(400, "POST", "/ceph/osds/1/explode")
    expect(400, "POST", "/ceph/osds/x/in")

    expect(200, "POST", "/ceph/pools", {"name": "newpool", "pg_num": 64, "size": 2, "application": "rbd"})
    log = cl()
    ok("osd pool create newpool 64" in log and "osd pool set newpool size 2" in log and "osd pool application enable newpool rbd" in log, "pool create runs create + size + application")
    expect(409, "POST", "/ceph/pools", {"name": "vmdata", "pg_num": 32})
    expect(422, "POST", "/ceph/pools", {"name": "bad pool", "pg_num": 32})
    expect(422, "POST", "/ceph/pools", {"name": "x", "pg_num": 33})
    expect(400, "DELETE", "/ceph/pools/vmdata")
    expect(400, "DELETE", "/ceph/pools/vmdata?confirm=other")
    expect(400, "DELETE", "/ceph/pools/.mgr?confirm=.mgr")
    expect(404, "DELETE", "/ceph/pools/nosuch?confirm=nosuch")
    expect(200, "DELETE", "/ceph/pools/vmdata?confirm=vmdata")
    ok("osd pool delete vmdata vmdata --yes-i-really-really-mean-it" in cl(), "pool delete needs its name twice, like Ceph")
    r = expect(200, "POST", "/ceph/flags/norebalance", {"set": True})
    ok("osd set norebalance" in cl(), "flag set runs `ceph osd set`")
    expect(200, "POST", "/ceph/flags/noout", {"set": False})
    ok("osd unset noout" in cl(), "flag clear runs `ceph osd unset`")
    expect(400, "POST", "/ceph/flags/everything", {"set": True})
    types = {t["task_type"] for t in expect(200, "GET", "/tasks?limit=200")}
    ok({"ceph-osd", "ceph-pool", "ceph-flag"} <= types, "Ceph changes are in the task log")

    print("[sysadmin-e2e] ALL OK", flush=True)


main()
