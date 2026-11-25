"""Ceph install from the Horcrux API on three installed nodes: pick a release, create the cluster on node 1, join nodes 2 and 3,
one OSD per node, HEALTH_OK, then an RBD pool that a VM's disk lives on (Horcrux, OpenRC and systemd).

Each node is a real ISO install (unattended answer file) with a second disk for its OSD and a second NIC on the hub `ceph`
(10.88.0.0/24, the Ceph public network). The manager under test is dropped in (`--arg bin=`), Ceph itself comes from the node's
package repositories through `POST /api/ceph/install` (a task), exactly as an administrator would do it from the UI.

    python3 -m vmtest run scenarios/ceph_install.py --arg iso=/iso/horcrux-server-openrc-daily-x86_64.iso \\
        --arg bin=/w/horcrux-api --arg cirros=/w/cirros.img --arg repo=/repo [--arg release=21] --out DIR

Needs a binary package of sys-cluster/ceph (and app-emulation/qemu[rbd]) on the configured binhost: a source build of Ceph
inside three 2-vCPU guests takes many hours, so the scenario refuses to start an install the catalog reports as source-only
(`--arg allow_source=1` overrides). Memory: 3 x `mem` (default 4096) + the runner; run it on a host with a 16 GiB+ slice.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Network, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

NET, PREFIX = "10.88.0", 24
NAMES = ["hx1", "hx2", "hx3"]
HTTP_PORT = 18098


def define(args):
    iso, binary, cirros, repo = args["iso"], args["bin"], args["cirros"], args["repo"]
    want = args.get("release", "")
    mem = int(args.get("mem", 4096))
    keys, nodes = {}, []
    for i, name in enumerate(NAMES, 1):
        answers, key = make_answers("horcrux")
        keys[name] = key
        nodes.append(NodeSpec(name=name, mem=mem + (1024 if i == 1 else 0), cpus=2, iso=iso, iso_media="usb", firmware="uefi",
                              disks=[Disk("24G"), Disk("12G")], extra_drives=[(answers, "usb")],
                              nics=[Nic("user", forwards={"ssh": 22, "web": 8006}), Nic("ceph", mac=f"52:54:00:0c:00:0{i}")],
                              ssh_key=key, console="none", boot_timeout=600, cpu="host"))

    def run(m):
        ns = [getattr(m, x) for x in NAMES]
        ip = {n.name: f"{NET}.1{i}" for i, n in enumerate(ns, 1)}

        m.phase("install three nodes")
        for n in ns:
            unattended_install(m, n)
        for n in ns:
            n.start()
        for n in ns:
            n.wait_for_ssh(600)
            n.wait_for_unit("horcrux-api", 300)
        m.check("three-nodes-up", True, ", ".join(f"{n.name}:{n.init.name}" for n in ns))

        m.phase("Ceph network + the manager under test")
        d = tempfile.mkdtemp(prefix="hxceph-")
        for src, nm in ((binary, "horcrux-api"), (cirros, "cirros.img"), (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py")):
            shutil.copy(src, os.path.join(d, nm))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(1)
            for i, n in enumerate(ns, 1):
                mac = f"52:54:00:0c:00:0{i}"
                n.succeed(f"dev=$(ip -o link | awk -F': ' '/{mac}/{{print $2}}' | cut -d@ -f1); ip link set $dev up; "
                          f"ip addr add {ip[n.name]}/{PREFIX} dev $dev")
                base = f"http://10.0.2.2:{HTTP_PORT}"
                n.succeed(f"curl -sf --retry 3 {base}/horcrux-api -o /tmp/horcrux-api.new && chmod 755 /tmp/horcrux-api.new; "
                          f"curl -sf --retry 3 {base}/hx_node.py -o /root/hx_node.py; mkdir -p /var/lib/horcrux/images; "
                          f"curl -sf --retry 3 {base}/cirros.img -o /var/lib/horcrux/images/cirros.img", timeout=900)
                n.stop_unit("horcrux-api")
                n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
                n.start_unit("horcrux-api")
                n.wait_until(lambda n=n: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")
            for a in ns:
                for b in ns:
                    if a is not b:
                        a.wait_until_succeeds(f"ping -c1 -W2 {ip[b.name]}", 60, 2)
            m.check("ceph-network", True, ", ".join(f"{k}={v}" for k, v in ip.items()))
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)

        env = f"HX_USER=hxadmin HX_PASS='{PW}' "

        def hx(n, method, path, body=None, ok=True, timeout=300):
            c = f"{env}python3 /root/hx_node.py api {method} {path}" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=timeout).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                raise AssertionError(f"{n.name} {method} {path}: {r}")
            return r

        def task(n, upid, secs):
            end = time.time() + n.T(secs)
            while time.time() < end:
                t = hx(n, "GET", f"/tasks/{upid}")["body"]
                if t.get("status") not in (None, "running"):
                    log = hx(n, "GET", f"/tasks/{upid}/log?start=0", ok=False)["body"]
                    return t, log
                time.sleep(15)
            raise AssertionError(f"{n.name}: task {upid} still running after {secs}s")

        m.phase("release catalog")
        cat = hx(ns[0], "GET", "/ceph/releases")["body"]
        majors = [r["major"] for r in cat["releases"] if r["available"]]
        m.check("catalog-lists-releases", bool(majors), json.dumps([(r["major"], r["newest"], r["repo"]) for r in cat["releases"]]))
        major = int(want) if want else cat.get("recommended") or max(majors)
        m.check("chosen-release-available", major in majors, f"release {major} of {majors}")
        unknown = hx(ns[0], "POST", "/ceph/install", {"release": "99"}, ok=False)
        m.check("unavailable-release-is-424", unknown["status"] == 424, str(unknown)[:300])
        binpkg = ns[0].execute("emerge -pq --usepkgonly sys-cluster/ceph >/dev/null 2>&1", timeout=600)[0] == 0
        if not binpkg and args.get("allow_source", "0") in ("", "0"):
            raise AssertionError("no binary package of sys-cluster/ceph on this node's binhost: a source build in the guests "
                                 "takes hours (pass --arg allow_source=1 to try anyway)")

        m.phase(f"install Ceph {major}: new cluster on hx1")
        r = hx(ns[0], "POST", "/ceph/install", {"release": str(major), "bootstrap": {
            "mode": "new", "public_network": f"{NET}.0/{PREFIX}", "mon_ip": ip["hx1"]}})
        t, log = task(ns[0], r["body"]["upid"], 14400)
        m.check("hx1-install-task-ok", t.get("exitstatus") == "OK", str(t) + "\n" + json.dumps(log)[-1500:])
        st = hx(ns[0], "GET", "/ceph/releases")["body"]
        m.check("hx1-installed-release", st.get("installed_major") == major and st.get("pinned") == major, json.dumps(st)[:300])

        m.phase("hx2 and hx3 join")
        for n in ns[1:]:
            r = hx(n, "POST", "/ceph/install", {"release": str(major), "bootstrap": {
                "mode": "join", "peer": f"{ip['hx1']}:8006", "username": "hxadmin", "password": PW, "mon_ip": ip[n.name]}})
            t, log = task(n, r["body"]["upid"], 14400)
            m.check(f"{n.name}-join-task-ok", t.get("exitstatus") == "OK", str(t) + "\n" + json.dumps(log)[-1500:])

        m.phase("one OSD per node, HEALTH_OK")
        for n in ns:
            dev = n.succeed("lsblk -dnpo NAME,SIZE | awk '$2==\"12G\"{print $1}' | head -1").strip()
            hx(n, "POST", "/ceph/osds", {"device": dev, "confirm": dev}, timeout=900)
        ns[0].wait_until(lambda: len([o for o in hx(ns[0], "GET", "/ceph/osds")["body"] if o.get("up")]) == 3, 900, 10, "3 OSDs up")
        mons = hx(ns[0], "GET", "/ceph/monitors")["body"]
        m.check("three-monitors", len(mons) == 3, json.dumps(mons)[:300])
        hx(ns[0], "POST", "/ceph/pools", {"name": "hxrbd", "pg_num": 32, "size": 3, "application": "rbd"})
        ns[0].wait_until(lambda: hx(ns[0], "GET", "/ceph/status")["body"].get("health", {}).get("status") == "HEALTH_OK", 900, 10,
                         "HEALTH_OK")
        m.check("cluster-health-ok", True, "3 mon, 3 osd, pool hxrbd")
        vers = hx(ns[0], "GET", "/system/versions")["body"]
        ceph_v = next((c for c in vers["components"] if c["id"] == "ceph"), {})
        m.check("about-reports-ceph", str(ceph_v.get("version", "")).startswith(f"{major}."), json.dumps(ceph_v))

        m.phase("a VM whose disk is on the RBD pool")
        hx(ns[0], "POST", "/storage/pools", {"name": "hxrbd", "storage_type": "ceph", "path": "hxrbd"})
        vm = "rbdvm"
        hx(ns[0], "POST", "/vms", {"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 256,
                                   "disk_size": 1, "status": "stopped", "display": {"protocol": "none"}, "hardware": {}})
        r = hx(ns[0], "POST", f"/vms/{vm}/disks", {"size_gb": 1, "storage": "hxrbd"}, ok=False)
        m.check("vm-disk-on-rbd-pool", r["status"] < 300 and "rbd:" in json.dumps(r["body"]), str(r)[:400])
        hx(ns[0], "POST", f"/vms/{vm}/start")
        ns[0].wait_until(lambda: hx(ns[0], "GET", f"/vms/{vm}")["body"]["status"] == "running", 300, 3, "VM running")
        blk = ns[0].succeed(f"{env}python3 /root/hx_node.py qmp /var/lib/horcrux/vms/{vm}.qmp.sock query-block")
        m.check("vm-runs-from-rbd", "rbd" in blk, blk[-400:])
        hx(ns[0], "POST", f"/vms/{vm}/stop", ok=False)
        for n in ns:
            n.power_off()
        for k in keys.values():
            shutil.rmtree(os.path.dirname(k), ignore_errors=True)

    return Test("ceph-install-3-nodes", nodes=nodes, networks=[Network("ceph")], run=run, timeout=6 * 3600,
                description="pick a Ceph release, install + bootstrap from the API, join 2 nodes, OSDs, HEALTH_OK, VM disk on RBD")
