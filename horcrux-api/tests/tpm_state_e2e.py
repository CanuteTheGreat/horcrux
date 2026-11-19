#!/usr/bin/env python3
"""Driven by tpm_state_e2e.sh: real swtpm + tpm2-tools + QEMU. Proves the TPM
that comes out of an import / backup / snapshot is the SAME TPM (a secret
sealed under its persistent key before the move unseals after it), and that
a missing state is refused instead of replaced by an empty TPM."""
import json
import os
import shutil
import signal
import ssl
import struct
import subprocess
import time
import urllib.error
import urllib.request

API = "https://127.0.0.1:8006/api"
CTX = ssl.create_default_context()
CTX.check_hostname = False
CTX.verify_mode = ssl.CERT_NONE
TOKEN = None
LV = "/tmp/lv"
UUID1 = "5880ef7c-4801-4911-88a7-79475a357d24"
UUID2 = "0b1c2d3e-4f50-4617-8899-aabbccddeeff"
SECRET = "the-secret-42"
PORT = [23100]


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


def sh(cmd, env=None, check_rc=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True, env={**os.environ, **(env or {})})
    if check_rc and r.returncode != 0:
        raise SystemExit(f"FAIL sh {cmd}\n{r.stdout}\n{r.stderr}")
    return r


class Swtpm:
    """A swtpm on a TCP port over a state directory (dir or linear file)."""

    def __init__(self, tpmstate):
        PORT[0] += 2
        self.port = PORT[0]
        self.pid = f"/tmp/lv/swtpm-{self.port}.pid"
        sh(
            f"swtpm socket --tpm2 --tpmstate {tpmstate} --server type=tcp,port={self.port} "
            f"--ctrl type=tcp,port={self.port + 1} --flags not-need-init,startup-clear "
            f"--daemon --pid file={self.pid} --log file=/tmp/lv/swtpm-{self.port}.log"
        )
        time.sleep(1)
        self.env = {"TPM2TOOLS_TCTI": f"swtpm:host=127.0.0.1,port={self.port}"}

    def tpm(self, cmd, check_rc=True):
        return sh(cmd, self.env, check_rc)

    def stop(self):
        sh(f"swtpm_ioctl --tcp 127.0.0.1:{self.port + 1} -s", check_rc=False)
        for _ in range(30):
            if not os.path.exists(f"/proc/{open(self.pid).read().strip()}") if os.path.exists(self.pid) else True:
                break
            time.sleep(0.2)
        time.sleep(0.3)


def seed(statedir=None, linear=None):
    """Manufacture a TPM and seal SECRET under a persistent primary key."""
    if linear:
        sh(f"truncate -s 4M {linear}")
        sh(f"swtpm_setup --tpm2 --tpmstate file://{linear} --createek --not-overwrite --config /dev/null")
        t = Swtpm(f"backend-uri=file://{linear}")
    else:
        os.makedirs(statedir, exist_ok=True)
        sh(f"swtpm_setup --tpm2 --tpmstate {statedir} --createek --not-overwrite --config /dev/null")
        t = Swtpm(f"dir={statedir}")
    d = os.path.dirname(linear or statedir)
    t.tpm(f"cd {d} && tpm2_createprimary -C o -c prim.ctx >/dev/null && tpm2_evictcontrol -C o -c prim.ctx 0x81000001 >/dev/null")
    t.tpm("tpm2_flushcontext -t || true", check_rc=False)
    t.tpm(f"cd {d} && echo -n {SECRET} | tpm2_create -C 0x81000001 -i- -u {LV}/s.pub -r {LV}/s.priv >/dev/null")
    t.stop()


def unseal(statedir):
    """Unseal SECRET from a COPY of the state directory (None: not the same TPM)."""
    tmp = f"/tmp/lv/unseal-{PORT[0]}"
    shutil.rmtree(tmp, ignore_errors=True)
    shutil.copytree(statedir, tmp)
    t = Swtpm(f"dir={tmp}")
    try:
        r = t.tpm(
            f"cd {tmp} && tpm2_load -C 0x81000001 -u {LV}/s.pub -r {LV}/s.priv -c s.ctx >/dev/null && tpm2_unseal -c s.ctx",
            check_rc=False,
        )
        return r.stdout.strip() if r.returncode == 0 else None
    finally:
        t.stop()


def domain_xml(name, uuid, disk, tpm=True):
    t = "<tpm model='tpm-crb'><backend type='emulator' version='2.0'/></tpm>" if tpm else ""
    return f"""<domain type='kvm'><name>{name}</name><uuid>{uuid}</uuid><memory unit='KiB'>262144</memory><vcpu>1</vcpu>
<os><type arch='x86_64' machine='pc-q35-8.0'>hvm</type></os>
<devices><disk type='file' device='disk'><driver name='qemu' type='qcow2'/><source file='{disk}'/><target dev='vda' bus='virtio'/></disk>
<interface type='network'><mac address='52:54:00:1E:2F:09'/><source network='default'/><model type='virtio'/></interface>
{t}</devices></domain>"""


def import_domain(domain, vm_id, **extra):
    _, r = call("POST", "/vms/import/libvirt", {"domain": domain, "id": vm_id, **extra})
    for _ in range(120):
        time.sleep(1)
        _, j = call("POST", "/vms/import/libvirt/status", {"job": r["job"]})
        if j["state"] != "running":
            return j
    raise SystemExit("FAIL import job did not finish")


def disk0(vm_id):
    return call("GET", f"/vms/{vm_id}")[1]["disks"][0]["path"]


def tpm_dir(vm_id):
    return os.path.splitext(disk0(vm_id))[0] + ".tpm"


def qemu_cmdline():
    return sh("ps -eo args | grep '[q]emu-system'", check_rc=False).stdout


_, r = call("POST", "/auth/login", {"username": "admin", "password": "admin"}, ok=(200,))
TOKEN = r.get("ticket") or r.get("token")

# ---- a libvirt domain with a real, used TPM (state under libvirt's swtpm root)
os.makedirs(f"{LV}/domains", exist_ok=True)
os.makedirs(f"{LV}/images", exist_ok=True)
src_state = f"{LV}/swtpm/{UUID1}/tpm2"
seed(statedir=src_state)
sh(f"qemu-img create -f qcow2 {LV}/images/win.qcow2 64M")
open(f"{LV}/domains/win11.xml", "w").write(domain_xml("win11", UUID1, f"{LV}/images/win.qcow2"))
check(unseal(src_state) == SECRET, "control: the secret unseals from the libvirt TPM state itself")

# ---- 1. import keeps the TPM: state, model, SMBIOS UUID, MAC
_, pv = call("POST", "/vms/import/libvirt/preview", {"domain": "win11"})
pl = pv["plan"]
check(pl["tpm_state"] and pl["tpm_model"] == "crb" and pl["tpm_blocker"] is None, "preview finds the libvirt TPM state and keeps tpm-crb")
j = import_domain("win11", "win11")
check(j["state"] == "done", f"libvirt import with TPM state succeeds ({j.get('error')})")
_, vm = call("GET", "/vms/win11")
hw = vm["hardware"]
check(hw["tpm"] and hw["tpm_model"] == "crb" and hw["tpm_state_expected"], "the VM keeps the CRB model and is marked as having TPM state")
check(hw["adv"]["smbios"]["uuid"] == UUID1, "the domain UUID is the VM's SMBIOS system UUID")
check(unseal(tpm_dir("win11")) == SECRET, "the secret sealed BEFORE the import unseals from the imported VM's TPM (same TPM)")
_, st = call("GET", "/vms/win11/tpm")
check(st["state_present"] and st["model"] == "crb" and not st["blocked"], "GET /tpm shows the model and that the state exists")

# the cleanup list names the old TPM directory only because the copy is verified
rec = json.load(open("/tmp/dev/import-cleanup/win11.json"))
items = {i["path"]: i for i in rec["items"]}
check(src_state in items and items[src_state]["requires"].endswith(".tpm"), "the cleanup offer lists the old TPM dir, requiring the verified copy")
_, off = call("POST", "/vms/import/cleanup/preview", {"vm_id": "win11"})
t = [i for i in off["items"] if i["path"] == src_state][0]
check(t["remove"], "with the verified copy in place the old TPM dir may be removed")
os.rename(tpm_dir("win11") + "/tpm2-00.permall", tpm_dir("win11") + "/tpm2-00.permall.hold")
_, off = call("POST", "/vms/import/cleanup/preview", {"vm_id": "win11"})
t = [i for i in off["items"] if i["path"] == src_state][0]
check(not t["remove"], "without the Horcrux TPM copy the old TPM dir is kept")
os.rename(tpm_dir("win11") + "/tpm2-00.permall.hold", tpm_dir("win11") + "/tpm2-00.permall")

# ---- 2. the imported VM really starts with its TPM (swtpm + QEMU tpm-crb)
call("POST", "/vms/win11/start")
time.sleep(2)
cl = qemu_cmdline()
check("tpm-crb" in cl and "tpmdev=tpm0" in cl, "QEMU runs the TPM as tpm-crb, like libvirt did")
call("POST", "/vms/win11/stop", ok=None)
time.sleep(2)
check(unseal(tpm_dir("win11")) == SECRET, "the TPM state still unseals after the VM ran")

# ---- 3. backup and restore carry the TPM
_, b = call("POST", "/backups", {"target_id": "win11", "mode": "stop", "compression": "zstd", "notes": "tpm"})
listing = sh(f"zstd -dc '{b['path']}' | tar -t").stdout
check("tpm-tpm2-00.permall" in listing, f"the backup archive contains the TPM state ({listing.split()})")
_, rr = call("POST", f"/backups/{b['id']}/restore", {"target_id": "win11-restored"})
check(unseal(tpm_dir("win11-restored")) == SECRET, "a restored VM unseals the secret: the same TPM")
_, rv = call("GET", "/vms/win11-restored")
check(rv["hardware"]["tpm_state_expected"] and rv["hardware"]["tpm_model"] == "crb", "the restored VM keeps model and the 'has state' mark")

# ---- 4. a disk-only snapshot carries the TPM and a rollback restores it
_, sn = call("POST", "/vms/win11/snapshots", {"name": "before", "include_memory": False})
sid = sn["id"]
shutil.copy(tpm_dir("win11") + "/tpm2-00.permall", f"{LV}/permall.before")
call("POST", "/vms/win11/tpm/reset", {"confirm_data_loss": True})
call("POST", "/vms/win11/start")
time.sleep(2)
call("POST", "/vms/win11/stop", ok=None)
time.sleep(2)
check(unseal(tpm_dir("win11")) is None, "after an explicit reset the VM has a NEW TPM (the secret is gone)")
call("POST", f"/vms/win11/snapshots/{sid}/restore", {})
check(unseal(tpm_dir("win11")) == SECRET, "rolling back the snapshot rolls the TPM back with the disk")

# ---- 5. a vanished state refuses the start; only the explicit reset starts over
shutil.rmtree(tpm_dir("win11"))
st, e = call("POST", "/vms/win11/start", ok=None)
msg = json.dumps(e)
check(st == 409 and "TPM state of VM win11 is missing" in msg and "BitLocker" in msg and "tpm/reset" in msg, f"start without the TPM state is refused with a clear error ({st})")
check(not os.path.exists(tpm_dir("win11")) and "swtpm" not in sh("ps -eo args | grep '[s]wtpm socket.*win11'", check_rc=False).stdout, "the refusal created no empty TPM")
_, st = call("GET", "/vms/win11/tpm")
check(st["blocked"] and not st["state_present"], "GET /tpm reports the VM as blocked")
code, _ = call("POST", "/vms/win11/tpm/reset", {}, ok=None)
check(code == 400, "creating a new TPM needs confirm_data_loss")
call("POST", "/vms/win11/tpm/reset", {"confirm_data_loss": True})
call("POST", "/vms/win11/start")
call("POST", "/vms/win11/stop", ok=None)
check(unseal(tpm_dir("win11")) is None, "after the explicit reset the VM starts with an empty TPM")

# ---- 6. a domain whose TPM state cannot be found is refused, not imported empty
open(f"{LV}/domains/lost.xml", "w").write(domain_xml("lost", UUID2, f"{LV}/images/win.qcow2"))
_, pv = call("POST", "/vms/import/libvirt/preview", {"domain": "lost"})
check(pv["plan"]["tpm_blocker"] and "state was not found" in pv["plan"]["tpm_blocker"], "preview names the missing TPM state")
j = import_domain("lost", "lost")
check(j["state"] == "failed" and "state was not found" in (j.get("error") or "") and "BitLocker" in j["error"], "the import is refused loudly")
check(call("GET", "/vms/lost", ok=None)[0] == 404, "nothing was created by the refused import")
j = import_domain("lost", "lost", accept_new_tpm=True)
check(j["state"] == "done", "with accept_new_tpm the import goes ahead")
check(call("GET", "/vms/lost")[1]["hardware"].get("tpm_state_expected", False) is False, "an accepted fresh TPM is a first-start TPM, not a lost one")

# ---- 7. a Proxmox tpmstate0 volume (swtpm linear file) becomes the VM's TPM
os.makedirs("/tmp/dev/images", exist_ok=True)
raw = "/tmp/dev/images/tpmstate0.raw"
seed(linear=raw)
check(raw and os.path.getsize(raw) >= 4096, "control: a swtpm linear-file TPM volume with a sealed secret")
call("POST", "/vms", {"id": "pve-vm", "name": "pve-vm", "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 128,
                      "disk_size": 1, "status": "stopped", "display": {"protocol": "none", "video": "std", "sound": "none"},
                      "hardware": {"nets": [], "tpm": True}})
check(call("GET", "/vms/pve-vm")[1]["hardware"]["tpm_model"] == "crb", "a new VM's TPM defaults to CRB")
code, _ = call("POST", "/vms/pve-vm/tpm/import-state", {"source": "/etc/passwd"}, ok=None)
check(code in (403, 400), "TPM state sources outside the image library/pools are refused")
call("POST", "/vms/pve-vm/tpm/import-state", {"source": raw})
check(unseal(tpm_dir("pve-vm")) == SECRET, "the Proxmox tpmstate0 volume unseals in the Horcrux VM: the same TPM")
code, _ = call("POST", "/vms/pve-vm/tpm/import-state", {"source": raw}, ok=None)
check(code == 409, "an existing TPM state is not overwritten without replace")

print("ALL OK")
