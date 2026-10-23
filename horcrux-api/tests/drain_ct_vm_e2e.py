#!/usr/bin/env python3
"""Cross-node container moves (drain / rebalance) end to end, for real.

Runs as root INSIDE VM node1 of drain_ct_vm_e2e.sh; node2 is a second VM on
10.77.0.2 (ssh with the same key). Both run the built horcrux-api, LXC, Incus,
Docker, Podman and CRIU, and know each other as cluster nodes.

For each runtime (lxc incus docker podman):
 A. DRAIN node1 (maintenance): the running container moves to node2 LIVE
    (CRIU; `incus move` for Incus): the in-memory marker + counter, a file in
    its filesystem and (Docker/Podman) a volume's data all survive; node1 keeps
    no copy; exactly one running copy in the cluster; the move is in the
    location history.
 B. DRAIN node2: it goes back to node1 (and a node in maintenance is no move
    target: node1 is not drained while node2 is).
 C. ROLLBACK (LXC/Docker/Podman): node2 has no room for the bundle; the move
    fails, the source resumes its own checkpoint (same marker) and node2 has
    nothing: never two copies, never zero.
 D. COLD: node2 has no CRIU; the container is stopped, copied and started
    there (filesystem + volume data survive, the processes restart).
"""
import json
import os
import re
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request

IP = {"node1": "10.77.0.1", "node2": "10.77.0.2"}
CTX = ssl._create_unverified_context()
TOKENS = {}
JWT = open("/root/jwt.secret").read().strip()
KEY = "/root/.ssh/hxvm"
COUNTER = "m=$(date +%s%N); i=0; while :; do i=$((i+1)); echo \"$i $m\" > /tmp/count; sleep 1; done"


def log(m):
    print(f"[ct-move] {m}", flush=True)


def check(c, m):
    if not c:
        log(f"FAIL: {m}")
        for n in ("node1", "node2"):
            log(f"---- {n} api.log")
            print(sh(n, "tail -60 /var/lib/hx/api.log | cut -c1-300", rc_ok=True), flush=True)
        sys.exit(1)
    log(f"ok: {m}")


def sh(node, cmd, rc_ok=False, timeout=600):
    if node == "node1":
        argv = ["bash", "-c", cmd]
    else:
        argv = ["ssh", "-i", KEY, "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
                "-o", "LogLevel=ERROR", f"root@{IP[node]}", cmd]
    p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    if p.returncode != 0 and not rc_ok:
        check(False, f"{node}: `{cmd}` -> {p.returncode}: {p.stderr.strip()} {p.stdout.strip()}")
    return (p.stdout + p.stderr).strip()


def api(node, method, path, body=None, expect=None, timeout=900):
    if node not in TOKENS:
        _, b = raw(node, "POST", "/auth/login", {"username": "admin", "password": "admin"}, None)
        TOKENS[node] = b.get("ticket") or b.get("token")
    st, b = raw(node, method, path, body, TOKENS[node], timeout)
    if expect is not None:
        check(st == expect, f"{node} {method} {path} -> {st} (expected {expect}): {json.dumps(b)[:500]}")
    return st, b


def raw(node, method, path, body, token, timeout=900):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(f"https://{IP[node]}:8006/api{path}", data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if token:
        r.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=CTX) as resp:
            st, text = resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        st, text = e.code, e.read().decode()
    try:
        return st, json.loads(text) if text else None
    except ValueError:
        return st, text


def wait(desc, fn, timeout=300):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            last = fn()
            if last:
                return last
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(2)
    check(False, f"timed out waiting for {desc} (last: {last})")


def start_api(node, extra=""):
    sh(node, f"printf '%s' '{extra}' > /var/lib/hx/extra-env; bash /usr/local/bin/hx-start-api.sh")
    TOKENS.pop(node, None)


def maint(node):
    return api(node, "GET", "/nodes/maintenance")[1]


def set_maintenance(node, on):
    api(node, "POST", "/nodes/maintenance", {"enabled": on, "drain": on}, expect=200)


def drained(node):
    def f():
        d = maint(node).get("drain") or {}
        return d if d.get("status") == "done" else None
    return wait(f"{node}'s drain to finish", f, 900)


class Rt:
    def __init__(self, name):
        self.name = name
        self.id = "mv" + name
        self.oci = name in ("docker", "podman")
        self.vol = f"hxvol-{name}"

    def create(self, node):
        body = {"id": self.id, "name": self.id, "runtime": self.name, "memory": 256, "cpus": 1, "status": "stopped"}
        if self.name == "docker":
            body.update(rootfs="alpine:3.20", command=["sh", "-c", COUNTER], volumes=[f"{self.vol}:/data"])
        elif self.name == "podman":
            body.update(rootfs="docker.io/library/alpine:3.20", command=["sh", "-c", COUNTER],
                        volumes=[f"{self.vol}:/data"])
        elif self.name == "incus":
            body["rootfs"] = "images:alpine/3.22"
        elif self.name == "lxc":
            _, cat = api(node, "GET", "/lxc/templates/catalog?arch=amd64")
            rel = max([e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
                       and re.fullmatch(r"\d+\.\d+", e["release"])],
                      key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
            api(node, "POST", "/lxc/templates", {"distro": "alpine", "release": rel})
            body["rootfs"] = f"alpine/{rel}"
            body["lxc"] = {"unprivileged": False, "features": {"checkpoint": True}}
        api(node, "POST", "/containers", body, expect=200)
        if self.name == "incus":
            sh(node, f"incus config set {self.id} security.privileged=true raw.lxc='lxc.console.path = none' "
                     "migration.stateful=true")
        api(node, "POST", f"/containers/{self.id}/start", {}, expect=200)

    def exec(self, node, cmd):
        st, b = api(node, "POST", f"/containers/{self.id}/exec", {"command": ["sh", "-c", cmd]})
        check(st == 200, f"exec on {node}: {cmd[:50]} -> {st} {b}")
        return b["output"].strip()

    def running(self, node):
        if self.name == "lxc":
            return "RUNNING" in sh(node, f"lxc-info -n {self.id} -s", rc_ok=True)
        if self.name == "incus":
            return "RUNNING" in sh(node, f"incus list {self.id} -f csv -c s", rc_ok=True).upper()
        return sh(node, f"{self.name} inspect -f '{{{{.State.Running}}}}' {self.id}", rc_ok=True) == "true"

    def exists(self, node):
        if self.name == "lxc":
            return sh(node, f"test -d /var/lib/lxc/{self.id} && echo y || echo n") == "y"
        if self.name == "incus":
            return self.id in sh(node, f"incus list {self.id} -f csv -c n", rc_ok=True)
        return sh(node, f"{self.name} inspect -f '{{{{.Name}}}}' {self.id} >/dev/null 2>&1 && echo y || echo n") == "y"

    def start_counter(self, node):
        if not self.oci:
            self.exec(node, "setsid nohup sh -c '" + COUNTER.replace("'", "'\\''") + "' </dev/null >/dev/null 2>&1 &")
        time.sleep(4)
        return self.read(node)

    def read(self, node):
        n, m = self.exec(node, "cat /tmp/count").split()
        return int(n), m

    def write_data(self, node, tag):
        self.exec(node, f"echo fs-{tag} > /root/hx-fs; sync")
        if self.oci:
            self.exec(node, f"echo vol-{tag} > /data/hx-vol; sync")

    def check_data(self, node, tag, what):
        check(self.exec(node, "cat /root/hx-fs") == f"fs-{tag}", f"{self.name}: filesystem data survived {what}")
        if self.oci:
            check(self.exec(node, "cat /data/hx-vol") == f"vol-{tag}", f"{self.name}: volume data survived {what}")


def copies(rt):
    """Running copies of the container in the whole cluster."""
    return sum(1 for n in ("node1", "node2") if rt.exists(n) and rt.running(n))


def moved_ok(node, rt, frm, to, reason="drain"):
    st, hist = api(node, "GET", f"/cluster/moves?guest={rt.id}")
    return any(h["from"] == frm and h["to"] == to and h["ok"] for h in hist if h["guest_id"] == rt.id), hist


def drain_and_check_result(rt, node):
    d = drained(node)
    res = {r["guest_id"]: r for r in d["results"]}
    return res.get(rt.id), d


def run_runtime(name):
    rt = Rt(name)
    log(f"=== {name}")
    rt.create("node1")
    n0, m0 = rt.start_counter("node1")
    rt.write_data("node1", "one")
    log(f"{name}: counter {n0}, marker {m0}")

    log(f"--- {name} A: drain node1 -> node2 (live)")
    st, plan = api("node1", "GET", "/nodes/maintenance/plan", expect=200)
    mv = [m for m in plan["moves"] if m["guest_id"] == rt.id]
    check(mv and mv[0]["to"] == "node2", f"{name}: the drain plan moves it to node2 ({plan})")
    set_maintenance("node1", True)
    r, d = drain_and_check_result(rt, "node1")
    check(r and r["status"] == "done", f"{name}: the move finished ({r})")
    check(not rt.exists("node1"), f"{name}: node1 has no copy any more")
    check(api("node1", "GET", f"/containers/{rt.id}")[0] == 404, f"{name}: node1's API has no record of it")
    check(rt.running("node2"), f"{name}: running on node2")
    st, c2 = api("node2", "GET", f"/containers/{rt.id}")
    check(st == 200 and c2["status"] == "running", f"{name}: node2's API tracks it as running")
    check(copies(rt) == 1, f"{name}: exactly one running copy in the cluster")
    time.sleep(3)
    n1, m1 = rt.read("node2")
    check(m1 == m0 and n1 > n0, f"{name}: LIVE: same process memory (marker {m0}), counter {n0} -> {n1}")
    rt.check_data("node2", "one", "the move")
    ok, hist = moved_ok("node1", rt, "node1", "node2")
    check(ok, f"{name}: location history has node1 -> node2 ({hist[:1]})")

    log(f"--- {name} B: drain node2 -> back to node1; maintenance nodes are no target")
    set_maintenance("node1", False)
    wait("node1 out of maintenance", lambda: not maint("node1")["enabled"], 60)
    wait("node2 sees node1 online", lambda: any(
        n["name"] == "node1" and n["status"].lower() == "online" for n in api("node2", "GET", "/cluster/nodes")[1]), 60)
    rt.write_data("node2", "two")
    set_maintenance("node2", True)
    r, d = drain_and_check_result(rt, "node2")
    check(r and r["status"] == "done", f"{name}: moved back ({r})")
    check(rt.running("node1") and not rt.exists("node2"), f"{name}: back on node1, node2 has no copy")
    check(copies(rt) == 1, f"{name}: still exactly one running copy")
    time.sleep(2)
    if name in ("incus", "podman"):
        # Incus (liblxc 6.0): an instance that was itself restored cannot be
        # checkpointed again. Podman: `restore --import` gives the container a
        # new id but CRIU puts the processes back in the source's cgroup scope,
        # which CRIU then refuses to dump again. Either way the way back falls
        # to stop -> move -> start.
        rt.check_data("node1", "two", "the move back (cold: a restored container is not checkpointable again)")
        rt.start_counter("node1")
        n2, m2 = rt.read("node1")
        m0, n1 = m2, n2
    else:
        n2, m2 = rt.read("node1")
        check(m2 == m0 and n2 > n1, f"{name}: still the same process ({n1} -> {n2})")
        rt.check_data("node1", "two", "the move back")
    st, plan = api("node1", "GET", "/nodes/maintenance/plan", expect=200)
    check(not plan["moves"] and any("maintenance" in u["reason"] for u in plan["unplaced"]),
          f"{name}: node2 in maintenance is no target for node1's drain ({plan['unplaced']})")
    set_maintenance("node2", False)
    wait("node2 out of maintenance", lambda: not maint("node2")["enabled"], 60)

    if name != "incus":
        log(f"--- {name} C: rollback when the target cannot take the bundle")
        sh("node2", "mkdir -p /mnt/hxsmall && mount -t tmpfs -o size=1m tmpfs /mnt/hxsmall")
        start_api("node2", "HORCRUX_MOVE_DIR=/mnt/hxsmall")
        wait("node1 sees node2", lambda: any(n["name"] == "node2" and n["status"].lower() == "online"
                                              for n in api("node1", "GET", "/cluster/nodes")[1]), 90)
        before = rt.read("node1")
        set_maintenance("node1", True)
        r, d = drain_and_check_result(rt, "node1")
        check(r and r["status"] == "failed", f"{name}: the move failed ({r and r.get('error')})")
        check(rt.running("node1") and not rt.exists("node2"), f"{name}: it runs on node1 again, node2 holds nothing")
        check(copies(rt) == 1, f"{name}: exactly one running copy after the failed move")
        time.sleep(3)
        nb, mb = rt.read("node1")
        check(mb == m0 and nb > before[0], f"{name}: and it is the original process (marker {mb}, {before[0]} -> {nb})")
        st, c1 = api("node1", "GET", f"/containers/{rt.id}")
        check(st == 200 and c1["status"] == "running", f"{name}: node1's API tracks it as running")
        set_maintenance("node1", False)
        wait("node1 out of maintenance", lambda: not maint("node1")["enabled"], 60)
        sh("node2", "umount /mnt/hxsmall")
        start_api("node2")

    log(f"--- {name} D: cold move (no CRIU on the target): stop -> copy -> start")
    start_api("node2", "HORCRUX_CRIU_BIN=/nonexistent/criu")
    wait("node1 sees node2", lambda: any(n["name"] == "node2" and n["status"].lower() == "online"
                                          for n in api("node1", "GET", "/cluster/nodes")[1]), 90)
    rt.write_data("node1", "three")
    set_maintenance("node1", True)
    r, d = drain_and_check_result(rt, "node1")
    check(r and r["status"] == "done", f"{name}: cold move finished ({r})")
    check(rt.running("node2") and not rt.exists("node1"), f"{name}: running on node2 only")
    check(copies(rt) == 1, f"{name}: exactly one running copy")
    rt.check_data("node2", "three", "the cold move")
    if rt.oci:
        # the container's own process restarted: a new marker, the counter from 1
        time.sleep(3)
        n3, m3 = rt.read("node2")
        check(m3 != m0 and n3 < n1 + 100, f"{name}: processes restarted on the target (marker {m3})")
    api("node2", "POST", f"/containers/{rt.id}/stop", {}, expect=None)
    api("node2", "DELETE", f"/containers/{rt.id}", None, expect=None)
    set_maintenance("node1", False)
    wait("node1 out of maintenance", lambda: not maint("node1")["enabled"], 60)
    start_api("node2")
    wait("node1 sees node2", lambda: any(n["name"] == "node2" and n["status"].lower() == "online"
                                          for n in api("node1", "GET", "/cluster/nodes")[1]), 90)
    log(f"{name}: PASSED")


def main():
    wanted = sys.argv[1:] or ["lxc", "incus", "docker", "podman"]
    log("== cluster: each node knows the other")
    for a, b in (("node1", "node2"), ("node2", "node1")):
        api(a, "DELETE", f"/cluster/nodes/{b}")
        api(a, "POST", f"/cluster/nodes/{b}", {"ip": IP[b]}, expect=201)
    wait("both nodes online", lambda: all(any(
        n["name"] == p and n["status"].lower() == "online" for n in api(a, "GET", "/cluster/nodes")[1])
        for a, p in (("node1", "node2"), ("node2", "node1"))), 120)
    check(True, "both nodes see each other online")
    for w in wanted:
        run_runtime(w)
    log("ALL CONTAINER MOVE CHECKS PASSED: " + ", ".join(wanted))


main()
