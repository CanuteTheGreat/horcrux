#!/usr/bin/env python3
"""HA failover end to end (driven by ha_failover_e2e.sh).

Three Horcrux nodes (sibling containers, shared volume as shared storage,
each with its own token secret), real QEMU:

 1. node1 CREATES the cluster through the API, node2 and node3 JOIN it with
    an administrator login; all three list three members and see each other
    online (the join handed over the cluster's service secret).
 2. A VM with its disk on shared storage is made an HA resource: the other
    nodes learn it (replicated definition + group).
 3. FAILURE: node1 is killed (QEMU and all). The surviving majority
    detects it, waits out the self-fence time and restarts the VM on
    another node; an HA event records the recovery.
 4. node1 COMES BACK with its stale record: it learns the VM moved and does
    not start a second copy.
 5. PARTITION: the node now running the VM loses the network. It
    SELF-FENCES (its QEMU is killed) and the VM is recovered elsewhere;
    after the partition heals there is still exactly one running copy.
 6. A guest/QEMU crash is restarted by HA; a VM the operator stops stays
    stopped.
 7. Fencing devices: add/list/remove through the API (secrets not echoed).
"""
import json
import os
import subprocess
import sys
import time

NODES = {k: os.environ[f"N{k}"] for k in (1, 2, 3)}
IPS = {k: os.environ[f"IP{k}"] for k in (1, 2, 3)}
NET = os.environ["NET"]
NAME = {k: f"node{k}" for k in (1, 2, 3)}
VM = "ha-vm"


def log(m):
    print(m, flush=True)


def check(c, what):
    if c:
        log(f"  ok: {what}")
    else:
        log(f"  FAIL: {what}")
        raise SystemExit(f"check failed: {what}")


def sh(k, cmd, ok=True):
    r = subprocess.run(["docker", "exec", NODES[k], "bash", "-c", cmd], capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit(f"node{k}: {cmd}: {r.stderr}{r.stdout}")
    return r.stdout.strip()


def api(k, method, path, body=None):
    args = ["docker", "exec", NODES[k], "python3", "/hx/hx_node.py", "api", method, path]
    if body is not None:
        args.append(json.dumps(body))
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"node{k} {method} {path}: {r.stderr}")
    out = json.loads(r.stdout)
    return out["status"], out["body"]


def wait(what, fn, timeout=120, every=2):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = fn()
            if last:
                log(f"  ok: {what}")
                return last
        except SystemExit:
            raise
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(every)
    raise SystemExit(f"timeout: {what} (last: {last})")


def qemu_running(k):
    return sh(k, "pgrep -f '[q]emu-system.*ha-vm' >/dev/null && echo y || echo n", ok=False) == "y"


def running_on(nodes=(1, 2, 3)):
    return [k for k in nodes if qemu_running(k)]


def ha_res(k):
    st, b = api(k, "GET", "/ha/status")
    if st != 200:
        return None
    for r in b["resources"]:
        if r["vm_id"] == VM:
            return r
    return None


def alive_nodes():
    out = []
    for k in (1, 2, 3):
        r = subprocess.run(["docker", "inspect", "-f", "{{.State.Running}}", NODES[k]], capture_output=True, text=True)
        if r.stdout.strip() == "true":
            out.append(k)
    return out


# --- 1. create / join --------------------------------------------------------
log("== 1. create + join a cluster through the API")
st, b = api(1, "POST", "/cluster/create", {"name": "hxha", "address": IPS[1]})
check(st == 201, f"node1 creates the cluster ({st} {b})")
st, b = api(1, "POST", "/cluster/create", {"name": "again", "address": IPS[1]})
check(st == 409, f"a second create is refused ({st})")
st, b = api(2, "POST", "/cluster/join", {"peer": IPS[1], "username": "admin", "password": "wrong", "address": IPS[2]})
check(st == 400, f"join with a wrong password is refused ({st})")
for k in (2, 3):
    st, b = api(k, "POST", "/cluster/join", {"peer": IPS[1], "username": "admin", "password": "admin", "address": IPS[k]})
    check(st == 200, f"node{k} joins ({st} {b})")


def all_online():
    for k in (1, 2, 3):
        st, nodes = api(k, "GET", "/cluster/nodes")
        if st != 200 or sorted(n["name"] for n in nodes) != ["node1", "node2", "node3"]:
            return False
        if any(n["status"].lower() != "online" for n in nodes):
            return False
    return True


wait("every node lists node1..3, all online (service tokens verify across different secrets)", all_online, 90)

# --- 2. HA resource ----------------------------------------------------------
log("== 2. HA resource on shared storage")
st, b = api(1, "POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 128,
    "disk_size": 1, "status": "stopped", "display": {"protocol": "none"}, "hardware": {},
})
check(st in (200, 201), f"VM created on node1 ({st} {b if st >= 300 else ''})")
sh(1, "qemu-img create -f qcow2 /shared/vms/ha-vm-data.qcow2 64M >/dev/null")
st, b = api(1, "GET", f"/vms/{VM}")
disk0 = b["disks"][0]["path"] if b.get("disks") else ""
check(disk0.startswith("/shared/vms/"), f"disk is on shared storage ({disk0})")
st, b = api(1, "POST", f"/vms/{VM}/start")
check(st == 200, f"VM started ({st} {b if st != 200 else ''})")
check(qemu_running(1), "QEMU runs on node1")
st, b = api(1, "POST", "/ha/groups", {"name": "g", "nodes": ["node1", "node2", "node3"], "restricted": False, "no_failback": False})
check(st in (200, 201), f"HA group ({st} {b})")
st, b = api(1, "POST", "/ha/resources", {"vm_id": VM, "group": "g", "priority": 2})
check(st == 201, f"VM added as HA resource ({st} {b})")
wait("node2 and node3 know the HA resource placed on node1",
     lambda: all((ha_res(k) or {}).get("current_node") == "node1" for k in (2, 3)), 60)

# --- 3. node failure ---------------------------------------------------------
log("== 3. node1 dies: the VM is restarted elsewhere")
t0 = time.time()
subprocess.run(["docker", "kill", NODES[1]], check=True, capture_output=True)
new = wait("VM runs on node2 or node3", lambda: running_on((2, 3)), 150)
log(f"  recovered after {int(time.time() - t0)}s on node{new[0]}")
check(len(new) == 1, "exactly one running copy")
owner = new[0]
r = ha_res(owner)
check(r["current_node"] == NAME[owner] and r["state"] == "Started", f"HA resource now placed on {NAME[owner]} ({r['current_node']}, {r['state']})")
st, ev = api(owner if owner != 1 else 2, "GET", "/ha/events")
check(any("recovered" in e["message"] for e in ev), "an HA event records the recovery")

# --- 4. node1 returns ----------------------------------------------------------
log("== 4. node1 comes back: no second copy")
subprocess.run(["docker", "start", NODES[1]], check=True, capture_output=True)
time.sleep(2)
sh(1, "bash /hx/start-api.sh")
wait("node1 learned the VM moved", lambda: (ha_res(1) or {}).get("current_node") == NAME[owner], 90)
time.sleep(10)
check(running_on() == [owner], f"only node{owner} runs the VM ({running_on()})")

# --- 5. partition -> self-fence ---------------------------------------------
log(f"== 5. node{owner} loses the network: self-fence + recovery")
subprocess.run(["docker", "network", "disconnect", NET, NODES[owner]], check=True, capture_output=True)
others = [k for k in (1, 2, 3) if k != owner]
wait(f"node{owner} killed its own VM (self-fence)", lambda: not qemu_running(owner), 90)
new2 = wait("VM recovered on a surviving node", lambda: running_on(others), 150)
check(len(new2) == 1, "exactly one running copy while partitioned")
subprocess.run(["docker", "network", "connect", NET, NODES[owner]], check=True, capture_output=True)
time.sleep(25)
check(len(running_on()) == 1 and running_on() == new2, f"after healing still one copy, on node{new2[0]} ({running_on()})")
owner = new2[0]
wait("the healed node adopted the new placement", lambda: all((ha_res(k) or {}).get("current_node") == NAME[owner] for k in (1, 2, 3)), 60)

# --- 6. crash restart / operator stop ---------------------------------------
log("== 6. crash restart and operator stop")
pid = sh(owner, "pgrep -f '[q]emu-system.*ha-vm' | head -1")
sh(owner, f"kill -9 {pid}")
wait("HA restarted the killed VM", lambda: qemu_running(owner) and sh(owner, "pgrep -f '[q]emu-system.*ha-vm' | head -1") != pid, 60)
st, b = api(owner, "POST", f"/vms/{VM}/stop")
check(st == 200, f"operator stops the VM ({st})")
time.sleep(15)
check(not qemu_running(owner), "a VM the operator stopped is not restarted by HA")
st, b = api(owner, "POST", f"/vms/{VM}/start")
check(st == 200 and qemu_running(owner), "operator starts it again")

# --- 7. fencing devices -----------------------------------------------------------
log("== 7. fencing devices")
st, b = api(1, "POST", "/ha/fencing/devices", {
    "id": "ipmi-n2", "agent_type": "Ipmi", "node": "node2", "address": "10.0.0.50",
    "username": "admin", "password": "s3cret", "port": None, "options": {}, "priority": 1})
check(st == 201 and "s3cret" not in json.dumps(b), f"device added, password not echoed ({st})")
st, b = api(1, "GET", "/ha/fencing/devices")
check(st == 200 and [d["id"] for d in b] == ["ipmi-n2"] and b[0]["has_password"], "device listed")
st, b = api(1, "DELETE", "/ha/fencing/devices/ipmi-n2")
check(st == 204, f"device removed ({st})")

# --- 8. Rustinion-facing: node summary, pause/resume, event webhooks -------------
log("== 8. node summary, pause/resume, persisted event webhooks")
recv = (
    "import http.server\n"
    "class H(http.server.BaseHTTPRequestHandler):\n"
    "    def do_POST(self):\n"
    "        n = int(self.headers.get('Content-Length', 0))\n"
    "        open('/tmp/hooks.log', 'a').write(self.rfile.read(n).decode() + '\\n')\n"
    "        self.send_response(200); self.end_headers()\n"
    "    def log_message(self, *a): pass\n"
    "http.server.HTTPServer(('0.0.0.0', 9000), H).serve_forever()\n"
)
recv_node = [k for k in (1, 2, 3) if k != owner][0]
sh(recv_node, "cat > /tmp/recv.py <<'EOF'\n" + recv + "EOF\nrm -f /tmp/hooks.log; (nohup python3 /tmp/recv.py >/dev/null 2>&1 &); sleep 1")
st, b = api(owner, "GET", "/nodes/summary")
check(st == 200 and b["node"] == NAME[owner] and b["version"], f"node summary answers ({st})")
check(b["cluster"]["name"] == "hxha" and b["cluster"]["nodes_online"] == 3 and b["cluster"]["quorum"] is True, f"summary: cluster hxha, 3 online, quorum ({b['cluster']})")
check(b["ha"]["enabled"] and b["ha"]["by_state"].get("Started") == 1 and b["vms"]["running"] == 1, f"summary: HA on, 1 started resource, 1 running VM ({b['ha']}, {b['vms']})")
st, hook = api(owner, "POST", "/webhooks", {
    "name": "qa", "url": f"http://{IPS[recv_node]}:9000/hook", "events": ["vm_state_changed", "ha_failover"],
    "secret": "qa-secret-0123456789", "retry_count": 0})
check(st in (200, 201), f"webhook created ({st} {hook})")
st, b = api(owner, "POST", f"/vms/{VM}/pause")
check(st == 200 and b["status"] == "paused", f"pause -> paused ({st} {b.get('status') if isinstance(b, dict) else b})")
st, b = api(owner, "GET", f"/vms/{VM}")
check(b["status"] == "paused", "the VM stays paused (state persisted)")
st, b = api(owner, "POST", f"/vms/{VM}/resume")
check(st == 200 and b["status"] == "running", f"resume -> running ({st})")
wait("the receiver got vm.state_changed for pause and resume",
     lambda: (lambda t: '"Paused"' in t and t.count("vm.state_changed") >= 2)(sh(recv_node, "cat /tmp/hooks.log", ok=False)), 30)
sh(owner, "bash /hx/start-api.sh")
st, hooks = api(owner, "GET", "/webhooks")
check(st == 200 and any(h.get("name") == "qa" or h.get("config", {}).get("name") == "qa" for h in hooks), "the webhook survived an API restart")
check(qemu_running(owner), "QEMU kept running through the API restart")
st, disks = api(owner, "GET", "/storage/disks")
check(st == 200 and isinstance(disks, list), f"node disks list answers ({st})")
st, sm = api(owner, "GET", "/storage/smart/disks")
check(st == 200 and isinstance(sm, list), f"SMART disk list answers without smartctl ({st})")
st, b = api(owner, "GET", "/storage/disks/no-such-disk")
check(st == 404, f"unknown disk id is 404 ({st})")
st, b = api(owner, "POST", "/storage/smart/disks/no-such-disk/test", {"test_type": "short"})
check(st == 404, f"SMART test on an unknown disk is 404 ({st})")

log("ALL OK")
