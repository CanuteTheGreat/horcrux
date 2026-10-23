#!/usr/bin/env python3
"""Container save-state end to end with REAL CRIU, for every runtime
(runs as root INSIDE a Debian VM that has LXC, Incus, Docker, Podman and CRIU;
see hx_vm.sh / guest_state_ct_vm_e2e.sh).

usage: guest_state_ct_vm_e2e.py [lxc incus docker podman]

For each runtime: a container runs a process that keeps a counter in memory
and a random marker taken at its start. Save state (checkpoint) stops the
container, Resume restores it: the marker and the counter continue (a fresh
start would show a new marker and a restarted counter), through the manual
actions and through the host shutdown / boot paths. `criu check` must pass:
this test never skips the CRIU half.
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

API = "https://127.0.0.1:8006/api"
CTX = ssl._create_unverified_context()
TOKEN = [None]
DATA = "/tmp/gsctvm-data"


def log(m):
    print(f"[gs-ct-vm] {m}", flush=True)


def fail(m):
    log(f"FAIL: {m}")
    subprocess.run("tail -80 /tmp/api.log", shell=True)
    sys.exit(1)


def check(c, m):
    if not c:
        fail(m)
    log(f"ok: {m}")


def req(method, path, body=None, expect=200, timeout=900):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(API + path, data=data, method=method)
    r.add_header("Content-Type", "application/json")
    if TOKEN[0]:
        r.add_header("Authorization", "Bearer " + TOKEN[0])
    try:
        with urllib.request.urlopen(r, timeout=timeout, context=CTX) as resp:
            status, text = resp.status, resp.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if expect is not None and status != expect:
        fail(f"{method} {path} -> {status} (expected {expect}): {text[:800]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def sh(cmd, rc_ok=False):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True, executable="/bin/bash")
    if p.returncode != 0 and not rc_ok:
        fail(f"`{cmd}` -> {p.returncode}: {p.stderr.strip()} {p.stdout.strip()}")
    return (p.stdout + p.stderr).strip()


def login():
    _, b = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = b.get("ticket") or b.get("token")


def start_api():
    sh(f"""if [ -f /tmp/api.pid ]; then kill $(cat /tmp/api.pid) 2>/dev/null || true; sleep 2; fi
cd {DATA}
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=8006 HORCRUX_DATA_DIR={DATA} HORCRUX_DATABASE_URL=sqlite://{DATA}/horcrux.db \
ADMIN_PASSWORD=admin HORCRUX_SSL_DIR={DATA}/ssl RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 \
RUST_LOG=info HORCRUX_BOOT_ID_FILE={DATA}/bootid HORCRUX_GUEST_BOOT_DELAY_SECS=3 \
nohup horcrux-api >> /tmp/api.log 2>&1 &
echo $! > /tmp/api.pid""")
    for _ in range(60):
        if sh("curl -skf https://127.0.0.1:8006/api/health >/dev/null && echo up", rc_ok=True) == "up":
            login()
            return
        time.sleep(1)
    fail("horcrux-api did not start")


def host_env():
    return (f"HORCRUX_DATA_DIR={DATA} HORCRUX_DATABASE_URL=sqlite://{DATA}/horcrux.db "
            f"HORCRUX_SSL_DIR={DATA}/ssl HORCRUX_BOOT_ID_FILE={DATA}/bootid")


def host_stop():
    return sh(f"{host_env()} horcrux-api guests stop")


def reboot_host():
    sh(f"echo boot-$RANDOM$RANDOM > {DATA}/bootid")
    start_api()


def wait_for(desc, fn, timeout=180):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:  # noqa: BLE001
            pass
        time.sleep(2)
    fail(f"timed out waiting for {desc}")


def guest(ct):
    _, o = req("GET", "/nodes/guest-power")
    return next(g for g in o["guests"] if g["id"] == ct or g.get("name") == ct), o


def exec_ct(ct, cmd):
    _, b = req("POST", f"/containers/{ct}/exec", {"command": ["sh", "-c", cmd]})
    return b["output"].strip()


COUNTER = "m=$(date +%s%N); i=0; while :; do i=$((i+1)); echo \"$i $m\" > /tmp/count; sleep 1; done"


class Rt:
    """Per-runtime knowledge: create, running?, counter."""

    def __init__(self, name):
        self.name = name
        self.id = "gs" + name
        self.oci = name in ("docker", "podman")

    def create(self):
        body = {"id": self.id, "name": self.id, "runtime": self.name, "memory": 256, "cpus": 1, "status": "stopped"}
        if self.name == "docker":
            body["rootfs"] = "alpine:3.20"
            body["command"] = ["sh", "-c", COUNTER]
        elif self.name == "podman":
            body["rootfs"] = "docker.io/library/alpine:3.20"
            body["command"] = ["sh", "-c", COUNTER]
        elif self.name == "incus":
            body["rootfs"] = "images:alpine/3.22"
        elif self.name == "lxc":
            _, cat = req("GET", "/lxc/templates/catalog?arch=amd64")
            rel = max([e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
                       and re.fullmatch(r"\d+\.\d+", e["release"])],
                      key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
            req("POST", "/lxc/templates", {"distro": "alpine", "release": rel})
            body["rootfs"] = f"alpine/{rel}"
            body["lxc"] = {"unprivileged": False, "features": {"checkpoint": True}}  # CRIU: privileged, no console/ttys
        req("POST", "/containers", body)

    def running(self):
        if self.name == "lxc":
            return "RUNNING" in sh(f"lxc-info -n {self.id} -s", rc_ok=True)
        if self.name == "incus":
            return "RUNNING" in sh(f"incus list {self.id} -f csv -c s", rc_ok=True).upper()
        return sh(f"{self.name} inspect -f '{{{{.State.Running}}}}' {self.id}", rc_ok=True) == "true"

    def stopped(self):
        return not self.running()

    def start_counter(self):
        if not self.oci:
            exec_ct(self.id, "setsid nohup sh -c '" + COUNTER.replace("'", "'\\''") + "' >/dev/null 2>&1 &")
        time.sleep(4)
        return self.read()

    def read(self):
        n, m = exec_ct(self.id, "cat /tmp/count").split()
        return int(n), m


def run_runtime(name):
    rt = Rt(name)
    log(f"=== {name}")
    rt.create()
    if name == "incus":
        g, _ = guest(rt.id)
        check(not g["can_save"] and "security.privileged" in (g["save_blocker"] or ""),
              f"incus: an ordinary instance is refused with the fix spelled out ({g['save_blocker'][:90]}...)")
        sh(f"incus config set {rt.id} security.privileged=true raw.lxc='lxc.console.path = none' migration.stateful=true")
    req("POST", f"/containers/{rt.id}/start", {})
    check(rt.running(), f"{name}: container running")
    n0, m0 = rt.start_counter()
    log(f"{name}: counter {n0}, marker {m0}")
    time.sleep(33)  # the API caches its CRIU probe for 30 s
    g, ov = guest(rt.id)
    check(ov["capabilities"]["criu"]["available"], f"{name}: API reports CRIU available ({ov['capabilities']['criu'].get('version')})")
    check(g["can_save"], f"{name}: guest can save state ({g.get('save_blocker')})")
    req("PUT", "/nodes/guest-power", {**ov["node"], "stop_action": "save", "start_action": "resume_if_running"})

    log(f"--- {name}: manual Save state / Resume")
    n0, m0 = rt.read()
    st, saved = req("POST", f"/containers/{rt.id}/save-state", {}, expect=None)
    check(st == 200, f"{name}: Save state -> {st} {json.dumps(saved)[:600]}")
    check(rt.stopped(), f"{name}: container stopped by the checkpoint")
    g, _ = guest(rt.id)
    check(g["saved"] is not None, f"{name}: guest list shows Saved")
    req("POST", f"/containers/{rt.id}/resume-state", {})
    check(rt.running(), f"{name}: Resume: running again")
    time.sleep(3)
    n1, m1 = rt.read()
    check(m1 == m0, f"{name}: same process memory restored (marker {m0})")
    check(n1 > n0, f"{name}: counter continued from its saved value ({n0} -> {n1}), it did not restart")
    check(guest(rt.id)[0]["saved"] is None, f"{name}: saved state consumed")

    if name == "incus":
        # LXC 6.0 limitation (cgroup2): an instance that was itself restored
        # cannot be checkpointed again ("Failed getting freezer path" in
        # liblxc). Save state refuses cleanly and leaves it running; a fresh
        # start makes it checkpointable again.
        st, b = req("POST", f"/containers/{rt.id}/save-state", {}, expect=None)
        check(st >= 400 and rt.running(), f"incus: a restored instance cannot be saved again, and is left running ({st})")
        req("POST", f"/containers/{rt.id}/stop", {})
        req("POST", f"/containers/{rt.id}/start", {})
        rt.start_counter()
        time.sleep(33)

    log(f"--- {name}: host shutdown / boot")
    n0, m0 = rt.read()
    out = host_stop()
    log("guests stop:\n  " + out.replace("\n", "\n  "))
    check(f"saved container {rt.id}" in out or "saved container" in out, f"{name}: host shutdown saved the container")
    check(rt.stopped(), f"{name}: stopped after the shutdown path")
    reboot_host()
    wait_for(f"{name}: container resumed by the boot path", rt.running)
    time.sleep(3)
    n1, m1 = rt.read()
    check(m1 == m0 and n1 > n0, f"{name}: boot path resumed the same process ({n0} -> {n1}, marker kept)")
    req("POST", f"/containers/{rt.id}/stop", {}, expect=None)
    req("DELETE", f"/containers/{rt.id}", None, expect=204)
    log(f"{name}: PASSED")


def main():
    wanted = sys.argv[1:] or ["lxc", "incus", "docker", "podman"]
    sh(f"mkdir -p {DATA}; echo boot-1 > {DATA}/bootid")
    chk = sh("criu check", rc_ok=True)
    check("Looks good" in chk, f"criu check passes on this kernel ({chk.splitlines()[-1] if chk else ''})")
    start_api()
    for w in wanted:
        run_runtime(w)
    log("ALL CONTAINER SAVE-STATE CHECKS PASSED (real CRIU): " + ", ".join(wanted))


main()
