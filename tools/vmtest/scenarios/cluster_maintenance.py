"""Node maintenance of any cluster member, Proxmox-style (Horcrux, OpenRC and systemd).

Three Horcrux nodes are installed from the real ISO (in parallel) and joined into one cluster over a private LAN. Node B carries HA
guests (two running CirrOS VMs, one stopped) and a running non-HA VM. Everything is driven from node A's API (the request is forwarded
to B over the cluster's peer channel):

  1. the plan of B's maintenance lists every guest with a target that is not B; a guest that cannot move (pinned to B) makes the
     enter call refuse with the reason unless forced;
  2. entering maintenance evacuates B: the running guests live-migrate to A/C (same QEMU process, guest uptime continues), the stopped
     HA guest moves offline, so HA places nothing on B any more;
  3. B is powered off while in maintenance: the cluster raises no node failure and no HA recovery, the guests keep running where they
     are; B boots again and is still in maintenance;
  4. leaving maintenance with return=origin brings every guest back to B (running ones live), exactly one copy of each in the cluster.

    python3 -m vmtest run scenarios/cluster_maintenance.py --arg iso=/iso/horcrux-server-openrc-daily-x86_64.iso \\
        --arg bin=/w/horcrux-api --arg cirros=/w/cirros.img --arg repo=/repo --out DIR

`bin` is the horcrux-api under test (built with every feature), `repo` the horcrux checkout (hx_node.py). About 4.5 GB RAM per node
(`--arg mem=`), nested KVM recommended.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Network, Nic, NodeSpec, Test, Disk  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

NAMES = ("hxa", "hxb", "hxc")
LAN = {"hxa": "10.88.0.11", "hxb": "10.88.0.12", "hxc": "10.88.0.13"}
MAC = {"hxa": "52:54:00:88:00:11", "hxb": "52:54:00:88:00:12", "hxc": "52:54:00:88:00:13"}
HTTP_PORT = 18098
GUESTS = ("g-run1", "g-run2", "g-off", "g-plain")     # on B: two running HA, one stopped HA, one running non-HA
HA_GUESTS = ("g-run1", "g-run2", "g-off")
RUNNING = ("g-run1", "g-run2", "g-plain")


def define(args):
    iso, binary, cirros, repo = args["iso"], args["bin"], args["cirros"], args["repo"]
    mem = int(args.get("mem", 4096))
    nodes = []
    for name in NAMES:
        answers, key = make_answers("horcrux", hostname=f"{name}.test.lan")
        nodes.append(NodeSpec(name=name, mem=mem, cpus=3, iso=iso, iso_media="usb", firmware="uefi", disks=[Disk("24G")],
                              extra_drives=[(answers, "usb")],
                              nics=[Nic("user", forwards={"ssh": 22, "web": 8006}), Nic("lan", mac=MAC[name])],
                              ssh_key=key, console="none", boot_timeout=600, cpu="host"))

    def run(m):
        N = {name: m.nodes[name] for name in NAMES}
        A, B, C = (N[x] for x in NAMES)

        m.phase("install three nodes in parallel")
        errs = {}

        def inst(n):
            try:
                unattended_install(m, n)
            except Exception as e:  # noqa: BLE001
                errs[n.name] = e
        ts = [threading.Thread(target=inst, args=(n,)) for n in N.values()]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        m.check("three-installs", not errs, repr(errs))

        def lan_up(n):
            """The LAN NIC (found by MAC) gets its cluster address (the management NIC stays on DHCP)."""
            ip = LAN[n.name]
            n.succeed(f"IF=$(ip -o link | awk -F': ' '/{MAC[n.name]}/ {{print $2}}' | cut -d@ -f1); "
                      f"ip link set $IF up; ip addr show dev $IF | grep -q '{ip}/' || ip addr add {ip}/24 dev $IF")

        m.phase("first boot, drop in the manager under test")
        d = tempfile.mkdtemp(prefix="hxsrv-")
        for src, fname in ((binary, "horcrux-api"), (cirros, "cirros.img"), (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py")):
            try:
                os.link(src, os.path.join(d, fname))
            except OSError:
                shutil.copy(src, os.path.join(d, fname))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        def boot(n):
            try:
                n.start()
                n.wait_for_ssh(900)
                n.wait_for_unit("horcrux-api", 300)
                base = f"http://10.0.2.2:{HTTP_PORT}"
                n.succeed("mkdir -p /var/lib/horcrux/images")
                for f, dst, mode in (("horcrux-api", "/tmp/horcrux-api.new", "755"), ("cirros.img", "/var/lib/horcrux/images/cirros.img", "644"),
                                     ("hx_node.py", "/root/hx_node.py", "755")):
                    n.succeed(f"curl -sf --retry 3 {base}/{f} -o {dst} && chmod {mode} {dst}", timeout=900)
                n.stop_unit("horcrux-api")
                n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
                n.start_unit("horcrux-api")
                n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 240, 3, "api health")
                lan_up(n)
            except Exception as e:  # noqa: BLE001
                errs[n.name] = e
        try:
            time.sleep(1)
            ts = [threading.Thread(target=boot, args=(n,)) for n in N.values()]
            for t in ts:
                t.start()
            for t in ts:
                t.join()
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)
        m.check("three-nodes-run-the-manager-under-test", not errs, repr(errs))
        m.check("init-system", A.init.name in ("openrc", "systemd"), A.init.name)

        env = f"HX_USER=hxadmin HX_PASS='{PW}' "

        def hx(n, method, path, body=None, ok=True, timeout=300):
            c = f"{env}python3 /root/hx_node.py api {method} '{path}'" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=timeout).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                raise AssertionError(f"{n.name} {method} {path}: {r}")
            return r

        names = {x: hx(N[x], "GET", "/nodes/maintenance")["body"]["node"] for x in NAMES}
        m.note("node-names", True, json.dumps(names))
        a, b, c = (names[x] for x in NAMES)

        m.phase("one cluster over the LAN")
        r = hx(A, "POST", "/cluster/create", {"name": "hxmaint", "address": LAN["hxa"]}, ok=False)
        m.check("cluster-created", r["status"] in (200, 201), json.dumps(r)[:300])
        for x in ("hxb", "hxc"):
            r = hx(N[x], "POST", "/cluster/join", {"peer": LAN["hxa"], "username": "hxadmin", "password": PW, "address": LAN[x]}, ok=False)
            m.check(f"{x}-joined", r["status"] == 200, json.dumps(r)[:300])

        def all_online():
            for x in NAMES:
                ns = hx(N[x], "GET", "/cluster/nodes")["body"]
                if sorted(n["name"] for n in ns if n["status"].lower() == "online") != sorted(names.values()):
                    return False
            return True
        A.wait_until(all_online, 240, 5, "every node sees all three online")
        m.check("all-three-online-everywhere", True, "")

        m.phase("guests on B: two running HA VMs, one stopped HA VM, one running non-HA VM")
        hx(B, "POST", "/ha/groups", {"name": "all", "nodes": list(names.values()), "restricted": False, "no_failback": False}, ok=False)
        for g in GUESTS:
            hx(B, "POST", "/vms", {"id": g, "name": g, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 192,
                                  "disk_size": 1, "status": "stopped", "display": {"protocol": "none"}, "hardware": {}})
            hx(B, "POST", f"/vms/{g}/import-disk", {"source": "/var/lib/horcrux/images/cirros.img"})
            hx(B, "POST", f"/vms/{g}/start")
        for g in HA_GUESTS:
            r = hx(B, "POST", "/ha/resources", {"vm_id": g, "group": "all", "priority": 2}, ok=False)
            m.check(f"ha-resource-{g}", r["status"] == 201, json.dumps(r)[:200])
        hx(B, "POST", "/vms/g-off/stop")

        def ha_res(n, g):
            for r in hx(n, "GET", "/ha/status")["body"].get("resources", []):
                if r["vm_id"] == g:
                    return r
            return None
        A.wait_until(lambda: all((ha_res(N[x], g) or {}).get("current_node") == b for x in ("hxa", "hxc") for g in HA_GUESTS),
                     180, 5, "A and C know B's HA guests")
        A.wait_until(lambda: (ha_res(B, "g-off") or {}).get("state") == "Stopped", 120, 3, "g-off is an intentionally stopped HA guest")
        m.check("ha-guests-known-cluster-wide", True, "")

        def where(g):
            """-> [(node, status)] for every node that has the guest."""
            out = []
            for x in NAMES:
                r = hx(N[x], "GET", f"/vms/{g}", ok=False)
                if r["status"] == 200:
                    out.append((names[x], r["body"]["status"]))
            return out

        def qemus(g):
            return sum(int(N[x].execute(f"pgrep -fc '[q]emu-system.*{g}' || true", 30)[1].strip() or 0) for x in NAMES)

        def guest_up(n, g):
            cfg = hx(n, "GET", f"/vms/{g}")["body"]
            sock = cfg["disks"][0]["path"].rsplit(".", 1)[0] + ".serial.sock"
            out = n.succeed(f"{env}python3 /root/hx_node.py serial {sock} \"cut -d' ' -f1 /proc/uptime\"", timeout=900).strip().splitlines()
            return float(out[-1].split()[-1])
        time.sleep(30)
        up0 = guest_up(B, "g-run1")
        m.note("g-run1-uptime-before", True, str(up0))

        m.phase("plan and refusal, asked of A")
        hx(B, "PUT", "/vms/g-plain/placement", {"pin": b})
        r = hx(A, "GET", f"/nodes/{b}/maintenance/plan")
        unplaced = {u["guest_id"]: u["reason"] for u in r["body"]["unplaced"]}
        m.check("plan-reports-the-pinned-guest", "pinned" in unplaced.get("g-plain", ""), json.dumps(r["body"])[:400])
        r = hx(A, "POST", f"/nodes/{b}/maintenance", {"enabled": True}, ok=False)
        m.check("enter-refused-while-a-guest-cannot-move", r["status"] == 409 and "g-plain" in json.dumps(r["body"]), json.dumps(r)[:300])
        m.check("refusal-changed-nothing", not hx(A, "GET", f"/nodes/{b}/maintenance")["body"]["enabled"], "")
        hx(B, "PUT", "/vms/g-plain/placement", {})
        r = hx(A, "GET", f"/nodes/{b}/maintenance/plan")["body"]
        targets = {mv["guest_id"]: mv["to"] for mv in r["moves"]}
        m.check("plan-moves-every-guest-off-b", sorted(targets) == sorted(GUESTS) and not r["unplaced"]
                and all(t in (a, c) for t in targets.values()), json.dumps(r)[:500])

        m.phase("ENTER maintenance of B from A")
        r = hx(A, "POST", f"/nodes/{b}/maintenance", {"enabled": True})
        m.check("enter-accepted-via-a", r["body"]["enabled"] is True and r["body"]["node"] == b, json.dumps(r["body"])[:300])

        def drained():
            v = hx(A, "GET", f"/nodes/{b}/maintenance")["body"]
            d = v.get("drain") or {}
            return v if d.get("status") == "done" and d.get("results") else None
        A.wait_until(drained, 1800, 10, "B's drain to finish")
        v = drained()
        assert v is not None
        res = {x["guest_id"]: x for x in v["drain"]["results"]}
        m.check("every-move-done", sorted(res) == sorted(GUESTS) and all(x["status"] == "done" for x in res.values()),
                json.dumps([(k, x["status"], x["error"]) for k, x in res.items()]))
        m.check("origins-recorded", sorted(o["guest_id"] for o in v["origins"]) == sorted(GUESTS)
                and all(o["origin"] == b for o in v["origins"]), json.dumps(v["origins"])[:400])
        for g in GUESTS:
            w = where(g)
            want = "running" if g in RUNNING else "stopped"
            m.check(f"{g}-off-b-{want}", len(w) == 1 and w[0][0] in (a, c) and w[0][1] == want, json.dumps(w))
        for g in RUNNING:
            m.check(f"{g}-one-qemu-in-cluster", qemus(g) == 1, str(qemus(g)))
        host1 = [x for x in NAMES if names[x] == where("g-run1")[0][0]][0]
        up1 = guest_up(N[host1], "g-run1")
        m.check("g-run1-moved-live-uptime-continued", up1 > up0, f"{up0} -> {up1}")
        for g in HA_GUESTS:
            A.wait_until(lambda g=g: (ha_res(A, g) or {}).get("current_node") in (a, c), 120, 5, f"HA home of {g} off B")
        m.check("ha-homes-off-b", all((ha_res(N[x], g) or {}).get("current_node") != b for x in ("hxa", "hxc") for g in HA_GUESTS), "")
        cm = hx(C, "GET", "/cluster/maintenance")["body"]
        row = next(n for n in cm["nodes"] if n["node"] == b)
        m.check("c-sees-b-in-maintenance", row["maintenance"]["enabled"] is True, json.dumps(row)[:300])
        p = hx(A, "GET", "/nodes/maintenance/plan")["body"]
        m.check("b-is-no-target-for-others", all(mv["to"] != b for mv in p["moves"]), json.dumps(p)[:300])

        m.phase("B goes down while in maintenance: no failure, no HA recovery")
        def recoveries():
            """HA failure/recovery events on the survivors (the list is newest first)."""
            out = []
            for x in ("hxa", "hxc"):
                out += [e for e in hx(N[x], "GET", "/ha/events?limit=1000")["body"] if e["event_type"] in ("Failed", "Recovered")]
            return out
        rec0 = len(recoveries())
        B.power_off(180)
        A.wait_until(lambda: any(n["name"] == b and n["status"].lower() != "online" for n in hx(A, "GET", "/cluster/nodes")["body"]),
                     300, 5, "A sees B offline")
        time.sleep(120)       # past the self-fence and recovery windows
        log_a = A.unit_log("horcrux-api", 400) + A.execute("grep -rh 'in maintenance' /var/log/horcrux/ 2>/dev/null | tail -20", 30)[1]
        m.check("b-down-reported-as-expected-not-failure", "went offline while in maintenance" in log_a, log_a[-1500:])
        rec = recoveries()
        m.check("no-ha-recovery-while-b-down", len(rec) == rec0, json.dumps(rec[: len(rec) - rec0])[:400])
        for g in RUNNING:
            w = where(g)
            m.check(f"{g}-still-running-off-b", len(w) == 1 and w[0][1] == "running" and w[0][0] != b, json.dumps(w))
        B.start()
        B.wait_for_ssh(900)
        B.wait_for_unit("horcrux-api", 300)
        lan_up(B)
        A.wait_until(lambda: any(n["name"] == b and n["status"].lower() == "online" for n in hx(A, "GET", "/cluster/nodes")["body"]),
                     300, 5, "A sees B online again")
        m.check("b-still-in-maintenance-after-boot", hx(A, "GET", f"/nodes/{b}/maintenance")["body"]["enabled"] is True, "")
        time.sleep(30)
        m.check("nothing-started-on-b-in-maintenance", int(B.execute("pgrep -c '[q]emu-system' || true", 30)[1].strip() or 0) == 0, "")

        m.phase("EXIT maintenance from A with return=origin")
        r = hx(A, "GET", f"/nodes/{b}/maintenance/plan?action=exit&return=origin")["body"]
        m.check("exit-plan-brings-all-back", sorted(mv["guest_id"] for mv in r["moves"]) == sorted(GUESTS)
                and all(mv["to"] == b for mv in r["moves"]), json.dumps(r)[:400])
        r = hx(A, "POST", f"/nodes/{b}/maintenance", {"enabled": False, "return": "origin"})
        m.check("exit-accepted", r["body"]["enabled"] is False, "")

        def returned():
            v = hx(A, "GET", f"/nodes/{b}/maintenance")["body"]
            e = v.get("exit") or {}
            return v if e.get("status") == "done" and e.get("results") else None
        A.wait_until(returned, 1800, 10, "the return to finish")
        v = returned()
        assert v is not None
        m.check("every-return-done", all(x["status"] == "done" for x in v["exit"]["results"]) and len(v["exit"]["results"]) == len(GUESTS),
                json.dumps([(x["guest_id"], x["status"], x["error"]) for x in v["exit"]["results"]]))
        for g in GUESTS:
            w = where(g)
            want = "running" if g in RUNNING else "stopped"
            m.check(f"{g}-back-on-b-{want}", w == [(b, want)], json.dumps(w))
        for g in RUNNING:
            m.check(f"{g}-one-qemu-after-return", qemus(g) == 1, str(qemus(g)))
        up2 = guest_up(B, "g-run1")
        m.check("g-run1-returned-live", up2 > up1, f"{up1} -> {up2}")
        for g in HA_GUESTS:
            A.wait_until(lambda g=g: (ha_res(A, g) or {}).get("current_node") == b, 120, 5, f"HA home of {g} back on B")
        m.check("ha-homes-back-on-b", True, "")
        hist = hx(B, "GET", "/cluster/moves")["body"]
        m.check("history-has-return-moves", sum(1 for h in hist if h["reason"] == "return" and h["to"] == b and h["ok"]) >= len(GUESTS),
                json.dumps(hist[:4])[:400])

    return Test("cluster-maintenance", nodes=nodes, networks=[Network("lan")], run=run, timeout=7200,
                description="maintenance of any member: evacuate HA guests, no recovery while down, return to origin")
