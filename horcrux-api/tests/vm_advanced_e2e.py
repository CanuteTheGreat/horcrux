#!/usr/bin/env python3
"""Driven by vm_advanced_e2e.sh: real QEMU, API calls checked against QMP, the
QEMU command line, the host's processes and files, and the hookscript's log."""
import json
import os
import pty
import socket
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
VM = "adv-e2e-vm"
POOL = "/tmp/dev/pool"
HOOKLOG = "/tmp/dev/hook.log"


def call(method, path, body=None, ok=(200, 201, 202, 204), raw=None, headers=None):
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    if TOKEN:
        req.add_header("Authorization", "Bearer " + TOKEN)
    try:
        with urllib.request.urlopen(req, context=CTX, timeout=240) as r:
            status, text = r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        status, text = e.code, e.read().decode()
    if ok is not None and status not in ok:
        raise SystemExit(f"FAIL {method} {path} -> {status}: {text}")
    try:
        return status, json.loads(text) if text else None
    except ValueError:
        return status, text


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL " + what)
    print("ok -", what, flush=True)


def qmp(sock_path, command, arguments=None):
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock_path)
    f = s.makefile("rw")
    f.readline()
    f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
    f.flush()
    f.readline()
    msg = {"execute": command}
    if arguments:
        msg["arguments"] = arguments
    f.write(json.dumps(msg) + "\n")
    f.flush()
    while True:
        line = json.loads(f.readline())
        if "return" in line or "error" in line:
            s.close()
            return line


def hmp(qsock, cmd):
    return qmp(qsock, "human-monitor-command", {"command-line": cmd})["return"]


def hooklog():
    return open(HOOKLOG).read().splitlines() if os.path.exists(HOOKLOG) else []


def qemu_cmdline():
    out = subprocess.run("ps -eo args | grep '[q]emu-system'", shell=True, capture_output=True, text=True).stdout
    return out


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

# --- a storage pool, the hookscript uploaded as a snippet, a share dir -------
os.makedirs(POOL + "/share", exist_ok=True)
open(POOL + "/share/hello.txt", "w").write("shared\n")
_, pool = call("POST", "/storage/pools", {"name": "advpool", "storage_type": "directory", "path": POOL})
pid_pool = pool["id"]
hook = ("#!/bin/sh\n"
        f"echo \"$1 $2\" >> {HOOKLOG}\n"
        "if [ \"$2\" = pre-start ] && [ -e /tmp/dev/hook-fail ]; then echo refused >&2; exit 7; fi\n")
st, item = call("POST", f"/storage/pools/{pid_pool}/upload?content=snippets&filename=hook.sh", raw=hook.encode(),
                headers={"Content-Type": "application/octet-stream"})
check(st == 201 and item["size"] == len(hook), "the hookscript is uploaded as a snippet")
HOOK = POOL + "/snippets/hook.sh"
check(os.access(HOOK, os.X_OK), "an uploaded snippet is executable")

# a host tty for the serial passthrough: a pty slave behind a /dev/tty* name
master, slave = pty.openpty()
TTY = "/dev/ttyHX0"
if os.path.lexists(TTY):
    os.remove(TTY)
os.symlink(os.ttyname(slave), TTY)

call("POST", f"/vms/{VM}/stop", ok=None)
call("DELETE", f"/vms/{VM}", ok=None)
hw = {
    "nets": [],
    "sockets": 2,
    "adv": {
        "serial": [{}, {"device": TTY}],
        "virtiofs": [{"tag": "share", "path": POOL + "/share"}],
        "ivshmem": {"name": "e2e", "size_mb": 4},
        "smbios": {"manufacturer": "Acme, Inc", "product": "Horcrux E2E", "serial": "SN-123"},
        "vmgenid": "auto",
        "numa": True,
        "max_cpus": 4,
        "max_memory_mb": 1024,
        "protection": True,
        "hookscript": HOOK,
    },
}

# validation first
def bad(adv_patch, what, code=(400, 422)):
    h = json.loads(json.dumps(hw))
    h["adv"].update(adv_patch)
    st, _ = call("POST", "/vms", {"id": "adv-bad", "name": "adv-bad", "hypervisor": "qemu", "architecture": "x86_64",
                                  "cpus": 2, "memory": 256, "disk_size": 1, "status": "stopped", "hardware": h}, ok=None)
    check(st in code, f"refused: {what} ({st})")
    call("DELETE", "/vms/adv-bad", ok=None)

bad({"virtiofs": [{"tag": "x", "path": "/etc"}]}, "virtiofs share outside the storage pools")
bad({"virtiofs": [{"tag": "bad tag", "path": POOL + "/share"}]}, "virtiofs tag with a space")
bad({"hookscript": "/bin/true"}, "hookscript that is not a snippet")
bad({"hookscript": "/etc/passwd"}, "hookscript /etc/passwd")
bad({"ivshmem": {"name": "e2e", "size_mb": 3}}, "ivshmem size that is not a power of two")
bad({"serial": [{"device": "/dev/sda"}]}, "serial passthrough of a non-tty")
bad({"serial": [{"device": "/dev/ttyNOPE9"}]}, "serial passthrough of a tty that does not exist")
bad({"smbios": {"serial": "x" * 65}}, "SMBIOS string over 64 characters")
bad({"max_cpus": 1}, "max CPUs below the CPU count")
bad({"max_memory_mb": 100}, "max memory below the VM's memory")
bad({"lock": "backup"}, "a lock other than manual")

_, vm = call("POST", "/vms", {
    "id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 2, "memory": 256, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "none", "video": "std", "sound": "none"},
    "hardware": hw,
})
genid = vm["hardware"]["adv"]["vmgenid"]
check(len(genid) == 36 and genid != "auto", f"vmgenid auto became a stored GUID ({genid})")

# protection: the VM can't be deleted
st, e = call("DELETE", f"/vms/{VM}", ok=None)
check(st == 409 and "protection" in json.dumps(e), "a protected VM refuses deletion")

# --- start: hook, devices, NUMA ------------------------------------------------
if os.path.exists(HOOKLOG):
    os.remove(HOOKLOG)
open("/tmp/dev/hook-fail", "w").close()
st, e = call("POST", f"/vms/{VM}/start", ok=None)
check(st >= 400 and "refused" in json.dumps(e) and "pre-start" in json.dumps(e), f"a failing pre-start hook stops the start ({st})")
check(call("GET", f"/vms/{VM}")[1]["status"] != "running" and not qemu_cmdline().strip(), "no QEMU is left behind")
check(subprocess.run("pgrep -x virtiofsd", shell=True).returncode != 0, "no virtiofsd is left behind")
os.remove("/tmp/dev/hook-fail")
call("POST", f"/vms/{VM}/start")
_, vm = call("GET", f"/vms/{VM}")
disk = vm["disks"][0]["path"]
base = os.path.splitext(disk)[0]
qsock = base + ".qmp.sock"
check(hooklog()[-2:] == [f"{VM} pre-start", f"{VM} post-start"], f"hookscript ran pre-start and post-start ({hooklog()})")

peri = {p["name"] for p in qmp(qsock, "qom-list", {"path": "/machine/peripheral"})["return"]}
check({"hxserdev0", "hxserdev1", "hxivshmem"} <= peri, f"serial ports and ivshmem are QEMU devices ({sorted(peri)})")
chard = hmp(qsock, "info chardev")
check("hxser0" in chard and "serial1.sock" in chard, "serial port 1 is a Unix socket chardev")
check(os.stat(base + ".serial1.sock").st_mode & 0o170000 == 0o140000, "its socket exists on the host")
check("hxser1: filename=serial" in chard, f"serial port 2 is an open host tty chardev ({chard!r})")
shm = "/dev/shm/hx-ivshmem-e2e"
check(os.path.exists(shm) and os.path.getsize(shm) == 4 * 1024 * 1024, "ivshmem backing file exists with the requested size")
qtree = hmp(qsock, "info qtree")
anon = qmp(qsock, "qom-list", {"path": "/machine/peripheral-anon"})["return"]
guids = []
for ent in anon:
    g = qmp(qsock, "qom-get", {"path": "/machine/peripheral-anon/" + ent["name"], "property": "guid"})
    if "return" in g:
        guids.append(g["return"])
check(genid in guids, f"VM Generation ID device carries the stored GUID ({guids})")
check("vhost-user-fs-pci" in qtree and "share" in qtree, "virtiofs device with its tag present")
check(subprocess.run("pgrep -x virtiofsd", shell=True).returncode == 0, "virtiofsd is serving the share")
cmd = qemu_cmdline()
check("-smbios type=1,manufacturer=Acme,, Inc,product=Horcrux E2E,serial=SN-123" in cmd, "SMBIOS strings are on the QEMU command line")
check(f"vmgenid,guid={genid}" in cmd, "the stored GUID is the one QEMU got")
numa = hmp(qsock, "info numa")
check("2 nodes" in numa and "node 0 cpus: 0" in numa.replace("\r", "") and "node 1 cpus:" in numa
      and "node 0 size: 128 MB" in numa and "node 1 size: 128 MB" in numa, f"NUMA: two nodes, 128 MB each ({numa!r})")
cpus = qmp(qsock, "query-cpus-fast")["return"]
check(len(cpus) == 2, "2 vCPUs at boot")

# --- hotplug ---------------------------------------------------------------------
st, e = call("POST", f"/vms/{VM}/hotplug/cpu", {"cpus": 9}, ok=None)
check(st in (400, 422), "more CPUs than max is refused")
st, e = call("POST", f"/vms/{VM}/hotplug/cpu", {"cpus": 2}, ok=None)
check(st in (400, 422), "hotplug only adds CPUs")
_, v = call("POST", f"/vms/{VM}/hotplug/cpu", {"cpus": 3})
check(v["cpus"] == 3, "the VM reports 3 CPUs")
cpus = qmp(qsock, "query-cpus-fast")["return"]
check(len(cpus) == 3, f"QEMU runs 3 vCPUs after hotplug ({len(cpus)})")
st, e = call("POST", f"/vms/{VM}/hotplug/memory", {"add_mb": 100}, ok=None)
check(st in (400, 422), "memory is added in multiples of 128 MB")
st, e = call("POST", f"/vms/{VM}/hotplug/memory", {"add_mb": 1024}, ok=None)
check(st in (400, 422), "memory above max is refused")
_, v = call("POST", f"/vms/{VM}/hotplug/memory", {"add_mb": 256})
check(v["memory"] == 512, "the VM reports 512 MB")
devs = qmp(qsock, "query-memory-devices")["return"]
check(len(devs) == 1 and devs[0]["data"]["size"] == 256 * 1024 * 1024, f"QEMU has a 256 MB DIMM ({devs})")

# --- lock (applies while running; start/stop/delete/config refuse) -------------
_, vm = call("GET", f"/vms/{VM}")
h = vm["hardware"]
h["adv"]["lock"] = "manual"
call("PUT", f"/vms/{VM}/config", {"hardware": h})
st, e = call("POST", f"/vms/{VM}/stop", ok=None)
check(st == 409 and "locked" in json.dumps(e), "a locked VM refuses stop")
st, e = call("PUT", f"/vms/{VM}/config", {"name": "renamed"}, ok=None)
check(st == 409 and "locked" in json.dumps(e), "a locked VM refuses config changes")
st, e = call("POST", f"/vms/{VM}/shutdown", ok=None)
check(st == 409 and "locked" in json.dumps(e), "a locked VM refuses shutdown")
h["adv"]["lock"] = None
call("PUT", f"/vms/{VM}/config", {"hardware": h})

# --- stop: hooks, virtiofsd gone, sockets gone -------------------------------------
call("POST", f"/vms/{VM}/stop")
check(hooklog()[-2:] == [f"{VM} pre-stop", f"{VM} post-stop"], f"hookscript ran pre-stop and post-stop ({hooklog()[-3:]})")
time.sleep(1)
check(subprocess.run("pgrep -x virtiofsd", shell=True).returncode != 0, "virtiofsd stopped with the VM")
check(not os.path.exists(base + ".serial1.sock") and not os.path.exists(base + ".virtiofs0.sock"), "the VM's extra sockets are cleaned up")

# --- persisted hotplug: restart boots with 3 CPUs and 512 MB ---------------------
call("POST", f"/vms/{VM}/start")
numa = hmp(qsock, "info numa")
check("node 0 size: 256 MB" in numa and "node 1 size: 256 MB" in numa, f"restart: 512 MB over two nodes ({numa!r})")
check(len(qmp(qsock, "query-cpus-fast")["return"]) == 3, "restart: 3 vCPUs")
call("POST", f"/vms/{VM}/stop")

# --- protection ----------------------------------------------------------------------
_, vm = call("GET", f"/vms/{VM}")
check(call("DELETE", f"/vms/{VM}", ok=None)[0] == 409, "still protected after use")
h = vm["hardware"]
h["adv"]["protection"] = False
call("PUT", f"/vms/{VM}/config", {"hardware": h})
call("DELETE", f"/vms/{VM}")
check(call("GET", f"/vms/{VM}", ok=None)[0] == 404, "an unprotected VM can be deleted")

# --- audio: a sound card over SPICE (QEMU's audiodev + the HDA/AC97 device) --------
AUD = "adv-audio-vm"
call("POST", f"/vms/{AUD}/stop", ok=None)
call("DELETE", f"/vms/{AUD}", ok=None)
for card, qdev in (("intel-hda", "intel-hda"), ("ich9-intel-hda", "ich9-intel-hda"), ("ac97", "AC97")):
    st, _ = call("POST", "/vms", {
        "id": AUD, "name": AUD, "hypervisor": "qemu", "architecture": "x86_64",
        "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
        "display": {"protocol": "spice", "video": "qxl", "sound": card},
        "hardware": {"nets": []},
    }, ok=None)
    check(st in (200, 201), f"VM with sound card {card} is created ({st})")
    call("POST", f"/vms/{AUD}/start")
    _, v = call("GET", f"/vms/{AUD}")
    aq = os.path.splitext(v["disks"][0]["path"])[0] + ".qmp.sock"
    qtree = hmp(aq, "info qtree")
    check(f'dev: {qdev}' in qtree, f"QEMU has the {qdev} sound device")
    if card != "ac97":
        check("hda-duplex" in qtree, "with an HDA duplex codec (playback + recording)")
    check("-audiodev spice,id=snd0" in qemu_cmdline(), "audio is carried over SPICE")
    call("POST", f"/vms/{AUD}/stop")
    call("DELETE", f"/vms/{AUD}")
# sound without SPICE is refused plainly
st, e = call("POST", "/vms", {
    "id": AUD, "name": AUD, "hypervisor": "qemu", "architecture": "x86_64",
    "cpus": 1, "memory": 128, "disk_size": 1, "status": "stopped",
    "display": {"protocol": "vnc", "video": "std", "sound": "intel-hda"}, "hardware": {"nets": []},
}, ok=None)
check(st in (400, 422) and "SPICE" in json.dumps(e), "a sound card on a VNC display is refused with the reason")
print("[vm-advanced-e2e] ALL OK", flush=True)
