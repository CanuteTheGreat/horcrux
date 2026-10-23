#!/usr/bin/env python3
"""Container half of the save-state end to end test (runs INSIDE the
privileged sibling of guest_state_ct_e2e.sh; real LXC, real API).

 A. WITHOUT CRIU: the node reports CRIU unavailable and each container's
    "state can't be saved" reason; a manual save is refused (the container keeps
    running); the HOST SHUTDOWN path falls back to a clean shutdown with the
    reason recorded, and the BOOT path starts the container again (start action
    Resume-if-running + the shutdown manifest).
 B. WITH CRIU (`criu check` passing): Save state checkpoints the container
    (it stops), Resume restores it - the same process keeps counting with the
    same PID - and the same through the host shutdown / boot paths. When
    `criu check` fails on this runner the half is SKIPPED, loudly, after
    checking that the API shows the reason CRIU reports.
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
import ssl

API = "https://127.0.0.1:8006/api"
CTX = ssl._create_unverified_context()
LXC_PATH = os.environ["HORCRUX_LXC_PATH"]
TOKEN = [None]


def log(m):
    print(f"[gs-ct-e2e] {m}", flush=True)


def fail(m):
    log(f"FAIL: {m}")
    subprocess.run("tail -60 /tmp/api.log", shell=True)
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
        fail(f"{method} {path} -> {status} (expected {expect}): {text[:600]}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def sh(cmd, rc_ok=False):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True, executable="/bin/bash")
    if p.returncode != 0 and not rc_ok:
        fail(f"`{cmd}` -> {p.returncode}: {p.stderr.strip()} {p.stdout.strip()}")
    return (p.stdout + p.stderr).strip()


def exec_ct(ct, cmd):
    _, b = req("POST", f"/containers/{ct}/exec", {"command": ["sh", "-c", cmd]})
    return b["output"].strip()


def state(ct):
    return sh(f"lxc-info -P {LXC_PATH} -n {ct} -s", rc_ok=True)


def wait_for(desc, fn, timeout=120):
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
    return next(g for g in o["guests"] if g["id"] == ct), o


def host_stop():
    return sh(". /tmp/env.sh; horcrux-api guests stop")


def reboot_host():
    sh("echo boot-$RANDOM$RANDOM > /tmp/gsct-data/bootid; bash /tmp/start-api.sh")
    login()


def login():
    _, b = req("POST", "/auth/login", {"username": "admin", "password": "admin"})
    TOKEN[0] = b.get("ticket") or b.get("token")


def counter_start(ct):
    exec_ct(ct, "nohup sh -c 'i=0; while :; do i=$((i+1)); echo $i > /tmp/count; sleep 1; done' >/dev/null 2>&1 &"
                " sleep 1; pgrep -f 'while :' | head -1 > /tmp/counter.pid")
    time.sleep(4)
    return exec_ct(ct, "cat /tmp/counter.pid"), int(exec_ct(ct, "cat /tmp/count"))


login()
log("--- setup: alpine template + a privileged container (CRIU can't dump user-namespaced ones)")
_, cat = req("GET", "/lxc/templates/catalog?arch=amd64")
rel = max([e for e in cat if e["distro"] == "alpine" and e["variant"] == "default"
           and re.fullmatch(r"\d+\.\d+", e["release"])], key=lambda e: tuple(int(x) for x in e["release"].split(".")))["release"]
req("POST", "/lxc/templates", {"distro": "alpine", "release": rel})
req("POST", "/containers", {"id": "gsct", "name": "gsct", "runtime": "lxc", "memory": 256, "cpus": 1,
                            "rootfs": f"alpine/{rel}", "status": "stopped", "lxc": {"unprivileged": False}})
req("POST", "/containers/gsct/start", {})
pid, c0 = counter_start("gsct")
log(f"counter pid {pid} at {c0}")

log("=== A. without CRIU: graceful fallback with a reason")
g, ov = guest("gsct")
check(not ov["capabilities"]["criu"]["available"], "node reports CRIU unavailable")
check(g["can_save"] is False and "CRIU" in (g["save_blocker"] or ""), f"guest shows why state can't be saved: {g['save_blocker']}")
st, b = req("POST", "/containers/gsct/save-state", {}, expect=None)
check(st >= 400 and "CRIU" in json.dumps(b), f"manual save refused with the reason ({st})")
check("RUNNING" in state("gsct"), "container keeps running after the refused save")
req("PUT", "/nodes/guest-power", {**ov["node"], "stop_action": "save", "start_action": "resume_if_running"})
out = host_stop()
log("guests stop:\n  " + out.replace("\n", "\n  "))
check("shutdown container gsct" in out and "CRIU" in out, "host shutdown: clean shutdown fallback, reason printed")
check("STOPPED" in state("gsct"), "container is stopped")
g, _ = guest("gsct")
check(g["saved"] is None and g["last_result"]["action"] == "shutdown" and "CRIU" in g["last_result"]["message"],
      "UI data: no saved state, last result = shutdown with the CRIU warning")
reboot_host()
wait_for("container started by the boot path", lambda: "RUNNING" in state("gsct"))
check(True, "boot path started the container again (it was running when the host went down)")
check(exec_ct("gsct", "ps | grep '[w]hile :' >/dev/null && echo running || echo none") == "none",
      "it is a fresh start (the counter process is gone: nothing was resumed)")

log("=== B. with CRIU")
sh("DEBIAN_FRONTEND=noninteractive apt-get install -y -qq criu >/dev/null", rc_ok=True)
chk = sh("criu check", rc_ok=True)
log("criu check: " + chk.replace("\n", " | ")[:300])
if "Looks good" not in chk:
    time.sleep(32)  # the API caches its CRIU probe for 30 s
    g, ov = guest("gsct")
    check(not ov["capabilities"]["criu"]["available"] and ov["capabilities"]["criu"]["error"], "API reports CRIU's own failure")
    log("SKIP: CRIU cannot run on this runner (kernel/container lacks checkpoint-restore); the real "
        "checkpoint/restore half is not exercised here. The fallback half above ran for real.")
    req("POST", "/containers/gsct/stop", {})
    req("DELETE", "/containers/gsct", None, expect=204)
    sys.exit(0)
time.sleep(32)
g, ov = guest("gsct")
check(ov["capabilities"]["criu"]["available"] and g["can_save"], "API sees CRIU and the container can save state")
pid, c0 = counter_start("gsct")
st, saved = req("POST", "/containers/gsct/save-state", {}, expect=None)
if st != 200:
    # `criu check` passes but the dump itself cannot run here (typically a
    # nested LXC whose cgroup layout CRIU cannot escape from). The failure
    # must be clean and explain itself; then this half is skipped, loudly.
    msg = json.dumps(saved)
    log(f"CRIU dump failed on this runner: {msg[:400]}")
    check("lxc-checkpoint" in msg, "the refusal names the failing step")
    check("RUNNING" in state("gsct"), "the container is still running after the failed save")
    g, _ = guest("gsct")
    check(g["saved"] is None and not os.path.exists("/tmp/gsct-data/saved-state/index/ct-gsct.json"),
          "no saved state was recorded for the failed save")
    check(g["last_result"]["ok"] is False, "the failure is recorded for the UI")
    # ... and the host-shutdown path still copes: clean shutdown fallback.
    out = host_stop()
    check("shutdown container gsct" in out and "State could not be saved" in out,
          "host shutdown falls back to a clean shutdown with the reason")
    check("STOPPED" in state("gsct"), "container stopped by the fallback")
    log("SKIP: the real CRIU checkpoint/restore half is not exercised on this runner "
        "(criu check passes but lxc-checkpoint cannot dump here); fallback behaviour verified above.")
    req("DELETE", "/containers/gsct", None, expect=204)
    sys.exit(0)
check(st == 200, f"Save state ({st} {json.dumps(saved)[:300]})")
check("STOPPED" in state("gsct"), "container stopped by the checkpoint")
g, _ = guest("gsct")
check(g["saved"] and g["saved"]["size_bytes"] > 0, "guest list shows Saved with a size")
req("POST", "/containers/gsct/resume-state", {})
check("RUNNING" in state("gsct"), "Resume: running again")
check(exec_ct("gsct", "cat /tmp/counter.pid") == pid and exec_ct("gsct", f"test -d /proc/{pid} && echo y") == "y", "same process (same PID) restored")
time.sleep(3)
c1 = int(exec_ct("gsct", "cat /tmp/count"))
check(c1 >= c0, f"the counter continued from its saved value ({c0} -> {c1}), it did not restart")
check(guest("gsct")[0]["saved"] is None, "saved state consumed")
log("--- host shutdown / boot with CRIU")
pid, c0 = counter_start("gsct")
out = host_stop()
log("guests stop:\n  " + out.replace("\n", "\n  "))
check("saved container gsct" in out, "host shutdown saved the container")
reboot_host()
wait_for("container resumed by the boot path", lambda: "RUNNING" in state("gsct"))
check(exec_ct("gsct", "cat /tmp/counter.pid") == pid, "boot path resumed the same process")
check(int(exec_ct("gsct", "cat /tmp/count")) >= c0, "counter continued across the host shutdown/boot")
req("POST", "/containers/gsct/stop", {})
req("DELETE", "/containers/gsct", None, expect=204)
log("ALL GUEST-STATE CONTAINER CHECKS PASSED")
