#!/usr/bin/env python3
"""Cluster drain / rebalance end to end (driven by drain_e2e.sh): two Horcrux
nodes (own network namespaces), real QEMU, real CirrOS guests.

 1. PLAN: draining node1 is previewed first: the movable VMs go to node2, the
    VM pinned to node1 stays, with the reason.
 2. DRAIN (Enter maintenance): the running VMs are live-migrated to node2
    WITHOUT a restart - a marker in guest RAM (/dev/shm), a running process and
    the uptime survive. node1 keeps no copy (exactly one QEMU per VM in the
    cluster), the moves are in the location history, a node in maintenance
    takes no migrations.
 3. HOST SHUTDOWN of node1 (`horcrux-api guests stop`, the unit's stop path):
    drains again (nothing left that can move), the pinned VM is saved to disk
    locally, node1's API goes away (the "shutdown completes").
 4. BOOT of node1: the pinned VM is resumed from its saved state, node1 leaves
    its shutdown maintenance and the cluster REBALANCES: a guest moves back
    to node1 (live, marker kept).
 5. A target that dies in the MIDDLE of a drain move: the move fails, the VM
    keeps running on the source, and once the target is back no second copy
    exists anywhere (one running QEMU for the VM in the whole cluster).
"""
import json
import os
import subprocess
import sys
import time

N1, N2 = os.environ["N1"], os.environ["N2"]
IP = {N1: os.environ["IP1"], N2: os.environ["IP2"]}
NAME = {N1: "node1", N2: "node2"}
NODE = {"node1": N1, "node2": N2}
STORE = "/var/lib/hx/vms"


def log(m):
    print(m, flush=True)


def check(cond, what):
    if cond:
        log(f"  ok: {what}")
    else:
        log(f"  FAIL: {what}")
        raise SystemExit(f"check failed: {what}")


def sh(node, cmd, ok=True):
    r = subprocess.run(["docker", "exec", node, "bash", "-c", cmd], capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit(f"{node}: {cmd}: {r.stderr}{r.stdout}")
    return r.stdout.strip()


def api(node, method, path, body=None):
    args = ["docker", "exec", node, "python3", "/hx/hx_node.py", "api", method, path]
    if body is not None:
        args.append(json.dumps(body))
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"{node} {method} {path}: {r.stderr}")
    out = json.loads(r.stdout)
    return out["status"], out["body"]


def guest(node, cfg, cmd):
    sock = cfg["disks"][0]["path"].rsplit(".", 1)[0] + ".serial.sock"
    r = subprocess.run(["docker", "exec", node, "python3", "/hx/hx_node.py", "serial", sock, cmd],
                       capture_output=True, text=True, timeout=1500)
    if r.returncode != 0:
        raise SystemExit(f"{node} guest `{cmd}`: {r.stderr}{r.stdout}")
    return r.stdout.strip()


def vm_cfg(node, vm):
    st, b = api(node, "GET", f"/vms/{vm}")
    return b if st == 200 else None


def uptime(node, cfg):
    return float(guest(node, cfg, "cut -d' ' -f1 /proc/uptime").split()[-1])


def create_vm(node, vm, memory=256, start=True):
    api(node, "DELETE", f"/vms/{vm}")
    st, b = api(node, "POST", "/vms", {"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
                                        "cpus": 1, "memory": memory, "disk_size": 1, "status": "stopped",
                                        "display": {"protocol": "none"}, "hardware": {}})
    check(st in (200, 201), f"create {vm} on {NAME[node]} ({st} {b if st >= 300 else ''})")
    st, b = api(node, "POST", f"/vms/{vm}/import-disk", {"source": "/var/lib/hx/images/cirros.img"})
    check(st in (200, 201), f"{vm}: CirrOS imported ({st})")
    if start:
        st, b = api(node, "POST", f"/vms/{vm}/start")
        check(st == 200, f"{vm} started ({st} {b if st != 200 else ''})")
    return vm_cfg(node, vm)


def qemus(vm):
    """Running QEMU processes of `vm` in the whole cluster."""
    n = 0
    for node in (N1, N2):
        out = sh(node, f"pgrep -f '[h]orcrux-vm-{vm}.pid' | wc -l")
        n += int(out or 0)
    return n


def wait(desc, fn, timeout=300):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = fn()
            if last:
                return last
        except SystemExit:
            raise
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(2)
    raise SystemExit(f"timed out waiting for {desc} (last: {last})")


def maint(node):
    return api(node, "GET", "/nodes/maintenance")[1]


def restart_api(node, extra_env=None):
    sh(node, f"printf '%s' '{extra_env or ''}' > /var/lib/hx/extra-env")
    sh(node, "bash /hx/start-api.sh")


# --- cluster membership ------------------------------------------------------
log("== cluster: each node knows the other")
for a, b in ((N1, N2), (N2, N1)):
    api(a, "DELETE", f"/cluster/nodes/{NAME[b]}")
    st, body = api(a, "POST", f"/cluster/nodes/{NAME[b]}", {"ip": IP[b]})
    check(st == 201, f"{NAME[a]} registers {NAME[b]} ({st} {body})")


def both_online():
    # Each node lists its peer (the local node may or may not be listed).
    for a, peer in ((N1, "node2"), (N2, "node1")):
        st, nodes = api(a, "GET", "/cluster/nodes")
        if st != 200 or not any(n["name"] == peer and n["status"].lower() == "online" for n in nodes):
            return False
    return True


wait("both nodes online", both_online, 120)
check(True, "both nodes see each other online")

# --- 1. plan ------------------------------------------------------------------
log("== 1. a drain plan before anything moves")
main = create_vm(N1, "dr-main", 256)
a = create_vm(N1, "dr-a", 128)
pinned = create_vm(N1, "dr-pin", 128)
marker = f"hx-drain-{int(time.time())}"
guest(N1, main, f"echo {marker} > /dev/shm/m; echo disk-{marker} > /home/cirros/d; sync; "
                "nohup sleep 77777 >/dev/null 2>&1 & echo $! > /dev/shm/sp")
pid = guest(N1, main, "cat /dev/shm/sp").split()[-1]
st, r = api(N1, "PUT", "/vms/dr-pin/placement", {"pin": "node1"})
check(st == 200 and r["pin"] == "node1", "dr-pin is pinned to node1")
st, r = api(N1, "PUT", "/vms/dr-pin/placement", {"pin": "nowhere"})
check(st == 422, f"pinning to a node that does not exist is refused ({st})")
time.sleep(20)
up_before = uptime(N1, main)
st, plan = api(N1, "GET", "/nodes/maintenance/plan")
check(st == 200, "GET maintenance/plan")
moved = {m["guest_id"]: m["to"] for m in plan["moves"]}
check(moved == {"dr-main": "node2", "dr-a": "node2"}, f"plan moves dr-main and dr-a to node2 ({moved})")
check([u["guest_id"] for u in plan["unplaced"]] == ["dr-pin"] and "pinned" in plan["unplaced"][0]["reason"],
      f"dr-pin stays, with the reason ({plan['unplaced']})")
check(vm_cfg(N1, "dr-main")["status"] == "running", "nothing moved by the dry run")

# --- 2. drain -----------------------------------------------------------------
log("== 2. Enter maintenance (drain): live moves, no restart")
st, m = api(N1, "POST", "/nodes/maintenance", {"enabled": True, "drain": True})
check(st == 200 and m["enabled"], f"maintenance on ({st})")


def drained():
    d = maint(N1).get("drain") or {}
    return d if d.get("status") == "done" and d.get("results") else None


d = wait("the drain to finish", drained, 1500)
res = {r["guest_id"]: r for r in d["results"]}
check(all(r["status"] == "done" for r in res.values()) and set(res) == {"dr-main", "dr-a"},
      f"both moves done ({[(k, v['status'], v['error']) for k, v in res.items()]})")
check(vm_cfg(N1, "dr-main") is None and vm_cfg(N1, "dr-a") is None, "node1 has no record of the moved VMs")
check(qemus("dr-main") == 1 and qemus("dr-a") == 1, "exactly one QEMU per moved VM in the cluster")
moved_main = vm_cfg(N2, "dr-main")
check(moved_main and moved_main["status"] == "running", "dr-main runs on node2")
check(guest(N2, moved_main, "cat /dev/shm/m").split()[-1] == marker, "RAM marker survived the move")
check(guest(N2, moved_main, f"test -d /proc/{pid} && echo alive").split()[-1] == "alive", "same guest process (PID) still running")
up_after = uptime(N2, moved_main)
check(up_after >= up_before and up_after < up_before + 900, f"uptime continued: {up_before} -> {up_after} (no restart)")
check(guest(N2, moved_main, "cat /home/cirros/d").split()[-1] == f"disk-{marker}", "disk data came along (local disks mirrored)")
check(vm_cfg(N1, "dr-pin")["status"] == "running", "pinned dr-pin still runs on node1")
st, hist = api(N1, "GET", "/cluster/moves")
mm = [h for h in hist if h["guest_id"] == "dr-main"]
check(mm and mm[0]["from"] == "node1" and mm[0]["to"] == "node2" and mm[0]["ok"] and mm[0]["reason"] == "drain",
      f"location history records node1 -> node2 (drain): {mm[:1]}")
st, nodes2 = api(N2, "GET", "/nodes/maintenance/plan")
check(st == 200 and all(m["to"] != "node1" for m in nodes2["moves"]), "a node in maintenance is no target for others")

st, s = api(N1, "PUT", "/cluster/rebalance", {"mode": "on_return", "max_moves": 2, "threshold_pct": 0.1, "min_interval_secs": 5})
check(st == 200, "rebalance settings: on return, 0.1 points threshold (replicated to node2)")

# --- 3. host shutdown -------------------------------------------------------------
log("== 3. host shutdown of node1: nothing movable left, the pinned VM is saved locally")
out = sh(N1, ". /hx/env.sh; horcrux-api guests stop")
log("    horcrux-api guests stop:\n      " + out.replace("\n", "\n      "))
check("saved vm dr-pin" in out, "pinned VM saved to disk (could not be moved)")
check("pinned" in out, "with the reason it stayed")
check(qemus("dr-pin") == 0, "no QEMU left on node1")
sh(N1, "kill $(cat /var/lib/hx/api.pid)")
time.sleep(3)
log("    node1's API is down (host powered off)")

# --- 4. boot + rebalance ---------------------------------------------------------
log("== 4. node1 boots: resumes its saved VM, leaves maintenance, the cluster rebalances")
sh(N1, "echo boot-2 > /var/lib/hx/bootid")
restart_api(N1)
wait("dr-pin resumed on node1", lambda: (vm_cfg(N1, "dr-pin") or {}).get("status") == "running", 300)
wait("node1 out of maintenance", lambda: not maint(N1)["enabled"], 120)
check(True, "node1 left its shutdown maintenance after boot")


def a_back():
    c = vm_cfg(N1, "dr-a")
    return c if c and c["status"] == "running" else None


back = wait("dr-a to be rebalanced back to node1", a_back, 900)
check(vm_cfg(N2, "dr-a") is None, "dr-a is gone from node2 (moved, not copied)")
check(qemus("dr-a") == 1, "one running copy of dr-a in the cluster")
check(float(uptime(N1, back)) < 3600, "dr-a is alive on node1 after the rebalance")
st, hist = api(N1, "GET", "/cluster/moves?guest=dr-a")
check(any(h["reason"] == "rebalance" and h["to"] == "node1" and h["ok"] for h in hist),
      f"history shows the rebalance move to node1: {hist[:2]}")
check(vm_cfg(N2, "dr-main")["status"] == "running", "dr-main stays on node2 (already even)")

# --- 5. target dies mid-move ---------------------------------------------------------
log("== 5. the target dies in the middle of a drain move")
restart_api(N1, "HORCRUX_DRAIN_BANDWIDTH_MBPS=1")  # slow moves, to catch one in flight
wait("node1 sees node2", both_online, 120)
st, plan = api(N1, "GET", "/nodes/maintenance/plan")
victim = plan["moves"][0]["guest_id"] if plan["moves"] else None
check(victim is not None, f"something to move: {plan['moves']}")
vcfg = vm_cfg(N1, victim)
vm_marker = f"hx-fail-{int(time.time())}"
guest(N1, vcfg, f"echo {vm_marker} > /dev/shm/f; sync")
st, m = api(N1, "POST", "/nodes/maintenance", {"enabled": True, "drain": True})
check(st == 200, "drain started")


def migrating():
    st, jobs = api(N1, "GET", "/migration/jobs")
    return any(j["status"] == "running" and j["progress"] >= 1 for j in jobs)


wait("a move to be in flight", migrating, 300)
log("    killing node2's API mid-move")
sh(N2, "kill -9 $(cat /var/lib/hx/api.pid)")


def move_failed():
    d = maint(N1).get("drain") or {}
    return d if d.get("status") == "done" else None


d = wait("the drain to give up", move_failed, 900)
failed = [r for r in d["results"] if r["status"] == "failed"]
check(failed, f"the interrupted move failed: {[(r['guest_id'], r['error']) for r in d['results']]}")
fid = failed[0]["guest_id"]
cfg = vm_cfg(N1, fid)
check(cfg and cfg["status"] == "running", f"{fid} keeps running on the source")
log("    node2 comes back")
restart_api(N2)
wait("no half-received copy of any VM on node2", lambda: all(
    qemus(v) <= 1 for v in ("dr-main", "dr-a", "dr-pin", victim, fid)), 180)
check(vm_cfg(N2, fid) is None, f"node2 has no record of {fid}")
check(qemus(fid) == 1, f"exactly one running QEMU for {fid} in the whole cluster")
check(guest(N1, vm_cfg(N1, fid), "cat /dev/shm/f 2>/dev/null || echo gone").split()[-1] in (vm_marker, "gone"),
      "and it is the original (never restarted)")

api(N1, "POST", "/nodes/maintenance", {"enabled": False, "drain": False})
for node in (N1, N2):
    st, vms = api(node, "GET", "/vms")
    for v in vms if st == 200 else []:
        api(node, "POST", f"/vms/{v['id']}/stop")
        api(node, "DELETE", f"/vms/{v['id']}")
log("ALL DRAIN / REBALANCE CHECKS PASSED")
