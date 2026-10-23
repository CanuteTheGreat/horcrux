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


def main():
    global TOKEN
    st, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"})
    assert st == 200, (st, r)
    TOKEN = r.get("ticket") or r.get("token")

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

    print("[sysadmin-e2e] ALL OK", flush=True)


main()
