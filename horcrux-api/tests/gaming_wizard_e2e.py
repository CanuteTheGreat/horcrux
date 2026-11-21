#!/usr/bin/env python3
"""Driven by gaming_wizard_e2e.sh: the Gaming VM wizard end to end on real
KVM. Fetches the Windows 11 and virtio-win ISOs through the ISO library
(unless already there), creates the VM with POST /api/vms/gaming (no GPU:
CI hosts have none), lets the unattended install run, waits for the server
to see the installed guest's agent (it deletes the answer disc then), and
checks from inside Windows over the guest agent: Secure Boot on, TPM 2.0
ready, VBS as the wizard composed it (running with Memory Integrity on a
bare-metal host; off on a host that is itself a VM, unless VBS=on), the
hypervisor visible (never hidden), the generated SMBIOS identity, the virtio
drivers and the account the wizard asked for."""
import base64
import http.client
import json
import os
import socket
import ssl
import sys
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
DATA = os.environ.get("HX_DATA", "/tmp/hxg")
INSTALL_MIN = int(os.environ.get("INSTALL_TIMEOUT_MIN", "100"))
POLL_GRACE_MIN = float(os.environ.get("POLL_GRACE_MIN", "10"))
T0 = time.time()


def call(method, path, body=None, ok=(200, 201, 202, 204)):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Content-Type", "application/json")
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


def mins():
    return (time.time() - T0) / 60


def qmp(sock, cmd):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(10)
    s.connect(sock)
    f = s.makefile("rw")
    f.readline()
    f.write(json.dumps({"execute": "qmp_capabilities"}) + "\n")
    f.flush()
    f.readline()
    f.write(json.dumps(cmd) + "\n")
    f.flush()
    while True:
        r = json.loads(f.readline())
        if "return" in r or "error" in r:
            s.close()
            return r


def qga(sock, cmd, timeout=15):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(sock)
    f = s.makefile("rw")
    sid = int(time.time() * 1000) % 1000000
    f.write(json.dumps({"execute": "guest-sync", "arguments": {"id": sid}}) + "\n")
    f.flush()
    while True:
        r = json.loads(f.readline())
        if r.get("return") == sid:
            break
    f.write(json.dumps(cmd) + "\n")
    f.flush()
    r = json.loads(f.readline())
    s.close()
    return r


def ps(sock, script):
    r = qga(sock, {"execute": "guest-exec", "arguments": {
        "path": "powershell.exe", "arg": ["-NoProfile", "-Command", script],
        "capture-output": True}}, 30)
    pid = r["return"]["pid"]
    for _ in range(180):
        st = qga(sock, {"execute": "guest-exec-status", "arguments": {"pid": pid}}, 30)["return"]
        if st.get("exited"):
            return base64.b64decode(st.get("out-data", "")).decode(errors="replace").strip()
        time.sleep(1)
    return "TIMEOUT"


def agent_up(sock):
    try:
        return "return" in qga(sock, {"execute": "guest-ping"}, 5)
    except Exception:
        return False


# ---------------------------------------------------------------- login
_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")
check(bool(TOKEN), "logged in")

_, settings = call("GET", "/vms/gaming/settings")
_, compat = call("GET", "/vms/gaming/compat")
check(len(compat["games"]) >= 20, f"compat table served ({len(compat['games'])} games)")

# ---------------------------------------------------- ISOs via the library
_, isos = call("GET", "/storage/isos")
have = {i["name"]: i for i in isos}
wanted = {
    os.environ.get("WIN_ISO_NAME", "win11-eval.iso"): settings["windows_eval_iso_url"],
    "virtio-win.iso": settings["virtio_win_url"],
}
for name, url in wanted.items():
    if name in have and not have[name].get("downloading"):
        print(f"ok - {name} already in the ISO library", flush=True)
        continue
    call("POST", "/storage/isos/download", {"url": url, "filename": name})
    print(f"... downloading {name} from {url}", flush=True)
deadline = time.time() + 60 * 60
while True:
    _, isos = call("GET", "/storage/isos")
    by = {i["name"]: i for i in isos}
    bad = [by[n]["error"] for n in wanted if n in by and by[n].get("error")]
    if bad:
        raise SystemExit(f"FAIL ISO download: {bad}")
    if all(n in by and not by[n].get("downloading") for n in wanted):
        break
    if time.time() > deadline:
        raise SystemExit("FAIL ISO downloads did not finish in 60 min")
    time.sleep(15)
win_iso = by[list(wanted)[0]]["path"]
virtio_iso = by["virtio-win.iso"]["path"]
check(True, f"ISO library has {win_iso} and {virtio_iso}")

# ------------------------------------------------------------- the wizard
req = {
    "name": "ci-gaming",
    "id": "ci-gaming",
    "gpu": None,
    "vcpus": int(os.environ.get("VCPUS", "4")),
    # 4 GiB = Windows 11's minimum: the CI job container is capped at 6 GiB
    # in total (QEMU overhead, swtpm and the API share it), and a 6 GiB guest
    # got QEMU OOM-killed by the job's memory cgroup during setup.
    "memory_mb": int(os.environ.get("MEMORY_MB", "4096")),
    "disk_gb": 64,
    "bridge": "",
    "hugepages": False,
    "windows_iso": win_iso,
    "virtio_iso": virtio_iso,
    "username": "player",
    "password": "Hx-ci-" + str(os.getpid()),
    "timezone": "Pacific Standard Time",
    "games": ["Counter-Strike 2", "Valorant"],
    "vbs": True,
    "start": False,
    "dry_run": True,
}
st, plan = call("POST", "/vms/gaming", req, ok=(200,))
hw = plan["config"]["hardware"]
check(hw["secure_boot"] and hw["tpm"] and hw["guest_agent"], "plan: Secure Boot, TPM 2.0, guest agent")
check(hw["adv"].get("nested_virt") is True, "plan: nested virtualisation for VBS")
check(hw["adv"]["smbios"]["manufacturer"] == settings["smbios_manufacturer"], "plan: Horcrux SMBIOS identity")
check(any(w.startswith("Valorant: blocked in VMs") for w in plan["warnings"]), "plan warns: Valorant blocked by its publisher")
check("password" not in json.dumps(plan["unattend"]), "plan never echoes the password")
host_is_vm = " hypervisor" in open("/proc/cpuinfo").read()
if host_is_vm:
    check(any("third level" in w for w in plan["warnings"]), "plan warns: VBS on a host that is itself a VM")
    no_vbs = dict(req, vbs=None)
    _, p2 = call("POST", "/vms/gaming", no_vbs, ok=(200,))
    # false is left out of the JSON (skip_serializing_if), so absent = off
    check(p2["config"]["hardware"]["adv"].get("nested_virt", False) is False,
          "VBS stays off by default when the host is itself a VM")
st, _ = call("GET", "/vms/ci-gaming", ok=None)
check(st == 404, "dry run created nothing")

# VBS for the real install. auto (default) leaves it to the wizard: on for a
# bare-metal host, off on a host that is itself a VM (Windows' hypervisor
# would run a third level deep). On the CI runners (AMD EPYC VMs, 4 CPUs per
# job) a VBS install boots into the third-level hypervisor and then crawls:
# no guest agent after 100 min (gaming-wizard-e2e run 22647, reproduced).
# VBS=on forces it (only on hosts known to cope), VBS=off turns it off.
VBS = os.environ.get("VBS", "auto")
if VBS not in ("auto", "on", "off"):
    raise SystemExit(f"FAIL VBS must be auto, on or off (got {VBS!r})")
req.update(dry_run=False, start=True, vbs={"auto": None, "on": True, "off": False}[VBS])
st, made = call("POST", "/vms/gaming", req, ok=(201,))
vm = made["config"]
disk = vm["disks"][0]["path"]
uuid = vm["hardware"]["adv"]["smbios"]["uuid"]
answer = made["unattend_iso"]
want_vbs = vm["hardware"]["adv"].get("nested_virt", False) is True
if VBS == "auto":
    check(want_vbs == (not host_is_vm),
          f"VBS {'on' if want_vbs else 'off'} by the wizard's default (host is {'a VM' if host_is_vm else 'bare metal'})")
else:
    check(want_vbs == (VBS == "on"), f"VBS {VBS} as asked")
check(os.path.isfile(answer) and (os.stat(answer).st_mode & 0o777) == 0o600, f"answer disc {answer} written 0600")
base = os.path.splitext(disk)[0]
qmp_sock, qga_sock = base + ".qmp.sock", base + ".qga.sock"
pidf = "/var/run/horcrux-vm-ci-gaming.pid"
cmdline = open(f"/proc/{open(pidf).read().strip()}/cmdline").read().replace("\0", " ") if os.path.exists(pidf) else ""
check(bool(cmdline), "QEMU is running")
if cmdline:
    open(f"{DATA}/qemu-cmdline.txt", "w").write(cmdline)
    intel = "GenuineIntel" in open("/proc/cpuinfo").read()
    nested = "vmx=on" if intel else "svm=on"
    for want in ("property=secure,value=on", "tpmdev", "drive=cdx1", "drive=cdunattend",
                 "hv-relaxed", "virtio-blk") + ((nested,) if want_vbs else ()):
        if want not in cmdline:
            print(cmdline)
        check(want in cmdline, f"QEMU runs with {want}")
    if not want_vbs:
        check(nested not in cmdline, f"QEMU runs without {nested} (VBS off)")
    check("kvm=off" not in cmdline and "hv_vendor_id" not in cmdline and "hv-vendor-id" not in cmdline,
          "hypervisor not hidden")

# --------------------------------------------- install until the agent answers
deadline = T0 + INSTALL_MIN * 60
unreachable_since = None
while os.path.exists(answer):
    if time.time() > deadline:
        try:
            qmp(qmp_sock, {"execute": "screendump", "arguments": {"filename": f"{DATA}/stuck.ppm"}})
        except Exception:
            pass
        raise SystemExit(f"FAIL Windows not installed (no guest agent) within {INSTALL_MIN} min; screendump {DATA}/stuck.ppm")
    # A dropped poll (connection reset, timeout) says nothing about the guest:
    # on a busy CI host the API can stall for a minute under memory pressure.
    # Only a reported non-running state, or the API staying unreachable for
    # POLL_GRACE_MIN, fails the test.
    try:
        _, v = call("GET", "/vms/ci-gaming")
    except (OSError, http.client.HTTPException) as e:
        if unreachable_since is None:
            unreachable_since = time.time()
        down = (time.time() - unreachable_since) / 60
        print(f"... status poll failed ({e!r}), API unreachable for {down:.1f} min", flush=True)
        if down > POLL_GRACE_MIN:
            raise SystemExit(f"FAIL API unreachable for {down:.0f} min during the install: {e!r}")
        time.sleep(15)
        continue
    unreachable_since = None
    if v["status"] not in ("Running", "running"):
        raise SystemExit(f"FAIL VM left running state: {v['status']}")
    time.sleep(30)
check(True, f"Windows installed and the server removed the answer disc ({mins():.0f} min)")
check(agent_up(qga_sock), "guest agent answers")
time.sleep(60)  # let first-logon commands settle


def inside():
    out = {}
    out["secure_boot"] = ps(qga_sock, "Confirm-SecureBootUEFI")
    out["tpm"] = ps(qga_sock, "(Get-Tpm | Select TpmPresent,TpmReady | ConvertTo-Json -Compress)")
    out["dg"] = ps(qga_sock, "(Get-CimInstance -Namespace root/Microsoft/Windows/DeviceGuard -ClassName Win32_DeviceGuard | Select VirtualizationBasedSecurityStatus,SecurityServicesRunning | ConvertTo-Json -Compress)")
    out["cs"] = ps(qga_sock, "(Get-CimInstance Win32_ComputerSystem | Select Manufacturer,Model,HypervisorPresent,Name | ConvertTo-Json -Compress)")
    out["uuid"] = ps(qga_sock, "(Get-CimInstance Win32_ComputerSystemProduct).UUID")
    out["virtio"] = ps(qga_sock, "(Get-PnpDevice -PresentOnly | ? { $_.FriendlyName -match 'VirtIO|Red Hat' } | Select -Expand FriendlyName) -join '; '")
    out["user"] = ps(qga_sock, "(Get-LocalUser player | Select Name,Enabled | ConvertTo-Json -Compress)")
    out["os"] = ps(qga_sock, "(Get-CimInstance Win32_OperatingSystem | Select Caption,BuildNumber | ConvertTo-Json -Compress)")
    for k, v in out.items():
        print(f"   {k}: {v}", flush=True)
    return out


o = inside()
dg = json.loads(o["dg"] or "{}")
if want_vbs and dg.get("VirtualizationBasedSecurityStatus") != 2:
    # VBS turned on during setup takes effect from the next boot
    print("... VBS not running yet, rebooting the guest once", flush=True)
    try:
        ps(qga_sock, "shutdown /r /t 5")
    except Exception:
        pass
    time.sleep(60)
    for _ in range(60):
        if agent_up(qga_sock):
            break
        time.sleep(10)
    time.sleep(60)
    o = inside()
    dg = json.loads(o["dg"] or "{}")
check(o["secure_boot"] == "True", "Secure Boot is on inside Windows")
tpm = json.loads(o["tpm"] or "{}")
check(tpm.get("TpmPresent") and tpm.get("TpmReady"), "TPM 2.0 present and ready inside Windows")
if want_vbs:
    check(dg.get("VirtualizationBasedSecurityStatus") == 2, "VBS running inside Windows (nested virtualisation)")
    check(2 in (dg.get("SecurityServicesRunning") or []), "Memory Integrity (HVCI) running")
else:
    check(dg.get("VirtualizationBasedSecurityStatus") != 2, "VBS not running inside Windows, as configured")
    hvci = ps(qga_sock, r"(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity' -Name Enabled -EA SilentlyContinue).Enabled")
    check(hvci != "1", f"the answer file did not turn Memory Integrity on (Enabled={hvci!r})")
cs = json.loads(o["cs"] or "{}")
check(cs.get("HypervisorPresent") is True, "Windows sees a hypervisor (never hidden)")
check(cs.get("Manufacturer") == settings["smbios_manufacturer"], f"SMBIOS manufacturer is {cs.get('Manufacturer')}")
check(o["uuid"].lower() == uuid.lower(), "SMBIOS UUID is the one generated for this VM")
check("VirtIO" in o["virtio"] or "Red Hat" in o["virtio"], "virtio drivers installed")
check('"Enabled":true' in o["user"].replace(" ", ""), "the wizard's account exists")
check(cs.get("Name") == "CI-GAMING", "computer name from the VM name")

# --------------------------------------------------------- stop, clean up
call("POST", "/vms/ci-gaming/stop")
check(not os.path.exists(answer), "answer disc stays deleted")
call("DELETE", "/vms/ci-gaming")
print(f"PASS gaming VM wizard: Windows 11 installed and verified in {mins():.0f} min", flush=True)
