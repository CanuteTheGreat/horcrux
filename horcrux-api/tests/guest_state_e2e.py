#!/usr/bin/env python3
"""Save state on host shutdown / resume on boot, QEMU VMs (driven by
guest_state_e2e.sh): real QEMU, real CirrOS guests, the real HTTP API and the
real `horcrux-api guests stop|boot` commands the systemd/OpenRC unit runs.

 1. manual Save state / Resume: a marker in guest RAM (/dev/shm) and a running
    process survive; the guest's uptime keeps counting (no reboot); the saved
    state is consumed (index + file gone) and cannot be resumed twice.
 2. not enough free space on the chosen storage: refused with the numbers, the
    VM keeps running untouched.
 3. HOST SHUTDOWN (the unit's stop path) with three guests: stop action Save
    saves, Shut down shuts down; then HOST BOOT (new boot id, API restarted,
    boot path): the saved guest resumes with the same RAM marker/process/uptime,
    the shut-down one boots again (disk marker, no RAM marker), an
    always-start guest that was stopped starts.
 4. UNCLEAN CRASH (QEMU killed, no manifest from a clean stop): the boot path
    follows the start action and starts the guest that was running.
 5. a saved state that no longer matches the disks is refused (kept, with the
    reason) and can be discarded, after which the VM boots normally.
"""
import json
import os
import subprocess
import sys
import time

N = os.environ["N1"]
FAILED = []


def log(m):
    print(m, flush=True)


def check(cond, what):
    if cond:
        log(f"  ok: {what}")
    else:
        log(f"  FAIL: {what}")
        FAILED.append(what)
        raise SystemExit(f"check failed: {what}")


def sh(cmd, ok=True):
    r = subprocess.run(["docker", "exec", N, "bash", "-c", cmd], capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit(f"{cmd}: {r.stderr}{r.stdout}")
    return (r.stdout + (r.stderr if not ok else "")).strip()


def api(method, path, body=None):
    args = ["docker", "exec", N, "python3", "/hx/hx_node.py", "api", method, path]
    if body is not None:
        args.append(json.dumps(body))
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"{method} {path}: {r.stderr}")
    out = json.loads(r.stdout)
    return out["status"], out["body"]


def guest(vm_cfg, cmd):
    sock = vm_cfg["disks"][0]["path"].rsplit(".", 1)[0] + ".serial.sock"
    r = subprocess.run(["docker", "exec", N, "python3", "/hx/hx_node.py", "serial", sock, cmd],
                       capture_output=True, text=True, timeout=1500)
    if r.returncode != 0:
        raise SystemExit(f"guest `{cmd}`: {r.stderr}{r.stdout}")
    return r.stdout.strip()


def vm_get(vm):
    return api("GET", f"/vms/{vm}")[1]


def uptime(cfg):
    return float(guest(cfg, "cut -d' ' -f1 /proc/uptime").split()[-1])


def create_vm(vm, start=True):
    api("DELETE", f"/vms/{vm}")
    st, b = api("POST", "/vms", {"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64",
                                  "cpus": 1, "memory": 256, "disk_size": 1, "status": "stopped",
                                  "display": {"protocol": "none"}, "hardware": {}})
    check(st in (200, 201), f"create {vm} ({st} {b if st >= 300 else ''})")
    st, b = api("POST", f"/vms/{vm}/import-disk", {"source": "/var/lib/hx/images/cirros.img"})
    check(st in (200, 201), f"{vm}: CirrOS imported ({st} {b if st >= 300 else ''})")
    if start:
        st, b = api("POST", f"/vms/{vm}/start")
        check(st == 200, f"{vm} started ({st} {b if st != 200 else ''})")
    return vm_get(vm)


def qemu_running(vm):
    return sh(f"pgrep -f '[h]orcrux-vm-{vm}.pid' || true") != ""


def wait_status(vm, want, timeout=240):
    end = time.time() + timeout
    while time.time() < end:
        st, b = api("GET", f"/vms/{vm}")
        if st == 200 and b["status"] == want:
            return True
        time.sleep(2)
    return False


def power():
    return api("GET", "/nodes/guest-power")[1]


def gview(vm):
    return next(g for g in power()["guests"] if g["id"] == vm)


def index_files():
    return sh("ls /var/lib/hx/saved-state/index 2>/dev/null; true")


def guests_cmd(what):
    return sh(f". /hx/env.sh; horcrux-api guests {what}")


def reboot_host():
    """What a reboot does to Horcrux: new boot id, API process gone and back."""
    sh("echo boot-$RANDOM$RANDOM > /var/lib/hx/bootid")
    sh("bash /hx/start-api.sh")


# ---------------------------------------------------------------- 1. manual
log("== 1. manual Save state / Resume with a marker in guest RAM")
v1 = create_vm("gs-save")
marker = f"hx-ram-{int(time.time())}"
guest(v1, f"echo {marker} > /dev/shm/m; echo disk-{marker} > /home/cirros/d; sync; "
          "nohup sleep 99999 >/dev/null 2>&1 & echo $! > /dev/shm/sleeppid")
sleep_pid = guest(v1, "cat /dev/shm/sleeppid").split()[-1]
time.sleep(20)
up_before = uptime(v1)
log(f"    guest uptime before: {up_before}s, sleep pid {sleep_pid}")
st, saved = api("POST", "/vms/gs-save/save-state")
check(st == 200, f"POST save-state ({st} {saved if st != 200 else ''})")
check(saved["size_bytes"] > 1_000_000 and saved["mechanism"].startswith("QEMU"), f"state file {saved['size_bytes']} bytes via {saved['mechanism']}")
check(not qemu_running("gs-save"), "QEMU process is gone after saving")
check(vm_get("gs-save")["status"] == "stopped", "VM is stopped")
g = gview("gs-save")
check(g["saved"] and g["saved"]["size_bytes"] == saved["size_bytes"], "guest list shows the saved state (size + time)")
check(int(sh(f"stat -c %s {saved['state_file']}")) == saved["size_bytes"], "state file on disk has the reported size")
st, _ = api("POST", "/vms/gs-save/save-state")
check(st == 409, f"saving a stopped/saved guest is refused ({st})")
st, b = api("POST", "/vms/gs-save/resume-state")
check(st == 200 and b["status"] == "running", f"resume-state ({st})")
check(qemu_running("gs-save"), "QEMU is running again")
v1 = vm_get("gs-save")
check(guest(v1, "cat /dev/shm/m").split()[-1] == marker, "RAM marker (/dev/shm) survived")
check(guest(v1, f"test -d /proc/{sleep_pid} && echo alive").split()[-1] == "alive", "the guest process (same PID) is still running")
up_after = uptime(v1)
check(up_after >= up_before and up_after < up_before + 600, f"uptime continued: {up_before} -> {up_after} (no reboot)")
check(index_files() == "", "saved state consumed (index gone)")
check(sh("ls /var/lib/hx/saved-state/data 2>/dev/null; true") == "", "state file deleted after the resume")
st, _ = api("POST", "/vms/gs-save/resume-state")
check(st in (404, 409), f"a second resume is refused ({st})")

# ------------------------------------------------------------ 2. free space
log("== 2. free-space check before saving")
sh("mkdir -p /mnt/small && mount -t tmpfs -o size=16m tmpfs /mnt/small")
st, b = api("POST", "/vms/gs-save/save-state", {"save_dir": "/mnt/small"})
check(st >= 400 and "Not enough free space" in json.dumps(b), f"save to a 16 MB storage is refused with the numbers ({st} {json.dumps(b)[:160]})")
check(qemu_running("gs-save") and vm_get("gs-save")["status"] == "running", "the VM keeps running")
check(guest(vm_get("gs-save"), "cat /dev/shm/m").split()[-1] == marker, "and its RAM is untouched")
check(index_files() == "", "nothing recorded for the refused save")
sh("umount /mnt/small")

# ------------------------------------------------------ 3. host shutdown/boot
log("== 3. HOST SHUTDOWN (unit stop path) then HOST BOOT (unit start path)")
v2 = create_vm("gs-shut")
guest(v2, "echo disk2 > /home/cirros/d2; echo ram2 > /dev/shm/m2; sync")
create_vm("gs-always", start=False)
st, _ = api("PUT", "/vms/gs-shut/power-policy", {"stop_action": "shutdown"})
check(st == 200, "gs-shut: stop action Shut down")
st, _ = api("PUT", "/vms/gs-always/power-policy", {"start_action": "always", "start_delay_secs": 1, "start_order": 5})
check(st == 200, "gs-always: start action Always start, delay 1, order 5")
st, node = api("PUT", "/nodes/guest-power", {**power()["node"], "stop_action": "save", "start_action": "resume_if_running"})
check(st == 200 and node["stop_action"] == "save", "node default: Save state / Resume if it was running")
marker = f"hx-host-{int(time.time())}"
v1 = vm_get("gs-save")
guest(v1, f"echo {marker} > /dev/shm/m3; nohup sleep 88888 >/dev/null 2>&1 & echo $! > /dev/shm/sp3")
sp3 = guest(v1, "cat /dev/shm/sp3").split()[-1]
time.sleep(15)
up_before = uptime(v1)
t0 = time.time()
out = guests_cmd("stop")
log("    horcrux-api guests stop:\n      " + out.replace("\n", "\n      "))
log(f"    stop path took {time.time()-t0:.0f}s")
check("saved vm gs-save" in out, "gs-save was saved")
check("shutdown vm gs-shut" in out, "gs-shut was shut down cleanly")
check(not qemu_running("gs-save") and not qemu_running("gs-shut"), "no QEMU is left running")
check(gview("gs-save")["saved"] is not None and gview("gs-shut")["saved"] is None, "only gs-save holds a saved state")
mf = json.loads(sh("cat /var/lib/hx/saved-state/running.json"))
check(mf["clean"] and {x["id"] for x in mf["guests"]} == {"gs-save", "gs-shut"}, "clean manifest lists what was running")
reboot_host()   # new boot id + API restart; the API's own boot pass runs
check(wait_status("gs-save", "running", 300), "gs-save is running after boot")
check(wait_status("gs-shut", "running", 300), "gs-shut is running after boot")
check(wait_status("gs-always", "running", 300), "gs-always (Always start, was stopped) is running after boot")
v1 = vm_get("gs-save")
check(guest(v1, "cat /dev/shm/m3").split()[-1] == marker, "RAM marker survived the host shutdown/boot")
check(guest(v1, f"test -d /proc/{sp3} && echo alive").split()[-1] == "alive", "same guest process still running")
up_after = uptime(v1)
check(up_after >= up_before and up_after < up_before + 900, f"uptime continued across the host reboot: {up_before} -> {up_after}")
check(index_files() == "", "saved states consumed by the boot")
v2 = vm_get("gs-shut")
check(guest(v2, "cat /home/cirros/d2").split()[-1] == "disk2", "gs-shut: disk data present after its clean shutdown + boot")
check(guest(v2, "cat /dev/shm/m2 2>/dev/null || echo none").split()[-1] == "none", "gs-shut: rebooted (RAM marker gone), as a Shut down guest should")
st, _ = api("POST", "/vms/gs-save/resume-state")
check(st in (404, 409), f"nothing left to resume twice ({st})")
r = guests_cmd("boot")
check("already ran" in r, f"`guests boot` is idempotent per boot ({r!r})")

# ------------------------------------------------------------ 4. crash path
log("== 4. unclean crash: QEMU killed, no clean stop")
time.sleep(25)   # the running-guests snapshot is refreshed every 20 s
mf = json.loads(sh("cat /var/lib/hx/saved-state/running.json"))
check(not mf["clean"] and "gs-save" in {x["id"] for x in mf["guests"]}, "periodic snapshot lists the running guests")
# The crash takes the API down with the guests (the periodic snapshot must
# not be refreshed after it): kill the API first, then every QEMU.
sh("kill -9 $(cat /var/lib/hx/api.pid)", ok=False)
sh("pkill -9 -f '[h]orcrux-vm-gs-save.pid'; pkill -9 -f '[h]orcrux-vm-gs-shut.pid'; pkill -9 -f '[h]orcrux-vm-gs-always.pid'; true", ok=False)
sh("rm -f /var/run/horcrux-vm-gs-*.pid")
reboot_host()
check(wait_status("gs-save", "running", 300), "after the crash, gs-save is started again by its start action")
v1 = vm_get("gs-save")
check(guest(v1, "cat /dev/shm/m3 2>/dev/null || echo none").split()[-1] == "none", "(a cold boot: nothing to resume after a crash)")

# --------------------------------------------- 5. stale state vs modified disk
log("== 5. a saved state that no longer matches its disks")
st, saved = api("POST", "/vms/gs-save/save-state")
check(st == 200, "saved again")
disk = vm_get("gs-save")["disks"][0]["path"]
sh(f"touch {disk}")
st, b = api("POST", "/vms/gs-save/resume-state")
check(st >= 400 and "modified since" in json.dumps(b), f"resume refused with the reason ({st} {json.dumps(b)[:200]})")
check(not qemu_running("gs-save"), "no half-started QEMU left behind")
g = gview("gs-save")
check(g["saved"] is not None, "the saved state was kept")
check(g["last_result"]["ok"] is False and "kept" in g["last_result"]["message"], "failure + 'kept' recorded for the UI")
st, _ = api("DELETE", "/vms/gs-save/saved-state")
check(st == 200, "discard")
check(index_files() == "", "index removed")
st, b = api("POST", "/vms/gs-save/start")
check(st == 200 and wait_status("gs-save", "running"), "the VM starts normally after discarding")

for vm in ("gs-save", "gs-shut", "gs-always"):
    api("POST", f"/vms/{vm}/stop")
    api("DELETE", f"/vms/{vm}")
log("ALL GUEST-STATE VM CHECKS PASSED")
