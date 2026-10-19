#!/usr/bin/env python3
"""Two-node VM migration end to end (driven by vm_migrate_e2e.sh).

Two Horcrux nodes in sibling containers (own network namespaces, one docker
network between them, a shared docker volume at /shared), real QEMU
(KVM when the runner has /dev/kvm) and real CirrOS guests:

 1. LIVE migration of a running UEFI (OVMF) + Q35 + TPM 2.0 VM with two
    disks (VirtIO + SCSI SSD) on node-local storage: disks mirrored over
    NBD, RAM/device state over the QEMU migration stream. The guest keeps
    running (uptime keeps counting, a file written before the move is
    there), TPM/EFI vars come along, the VM's firewall rule moves with it,
    nothing is left on the source.
 2. OFFLINE migration of that running VM back (shut down, disks + EFI vars
    + TPM state copied in chunks and SHA-256 verified, started on the
    target): the file is still there after the reboot.
 3. CANCEL of a slow (1 MB/s) live migration: the job ends "cancelled", the
    VM keeps running on the source in the same QEMU process, the target has
    no trace of it.
 4. A target that can't run the VM (bridge missing there): the job fails
    with the reason, the VM stays where it was.
 5. SHARED storage: live migration of a VM whose disk is on storage both
    nodes see - nothing is copied, the same file is used by the target.
"""
import json
import os
import subprocess
import sys
import time

N1, N2 = os.environ["N1"], os.environ["N2"]
IP = {N1: os.environ["IP1"], N2: os.environ["IP2"]}
NAME = {N1: "node1", N2: "node2"}
STORE = "/var/lib/hx/vms"
FAILED = []


def log(msg):
    print(msg, flush=True)


def check(cond, what):
    if cond:
        log(f"  ok: {what}")
    else:
        log(f"  FAIL: {what}")
        FAILED.append(what)
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


def qmp(node, disk0, command, arguments=None):
    sock = disk0.rsplit(".", 1)[0] + ".qmp.sock"
    args = ["docker", "exec", node, "python3", "/hx/hx_node.py", "qmp", sock, command]
    if arguments:
        args.append(json.dumps(arguments))
    r = subprocess.run(args, capture_output=True, text=True)
    return json.loads(r.stdout) if r.returncode == 0 else {"error": r.stderr}


def guest(node, disk0, cmd):
    sock = disk0.rsplit(".", 1)[0] + ".serial.sock"
    r = subprocess.run(
        ["docker", "exec", node, "python3", "/hx/hx_node.py", "serial", sock, cmd],
        capture_output=True,
        text=True,
        timeout=900,
    )
    if r.returncode != 0:
        raise SystemExit(f"{node} guest `{cmd}`: {r.stderr}{r.stdout}")
    return r.stdout.strip()


def uptime(node, disk0):
    return float(guest(node, disk0, "cut -d' ' -f1 /proc/uptime").split()[-1])


def create_vm(node, vm, hardware=None, disks=None, start=True):
    api(node, "DELETE", f"/vms/{vm}")
    body = {
        "id": vm,
        "name": vm,
        "hypervisor": "qemu",
        "architecture": "x86_64",
        "cpus": 1,
        "memory": 256,
        "disk_size": 1,
        "status": "stopped",
        "display": {"protocol": "none"},
        "hardware": hardware or {},
    }
    if disks:
        body["disks"] = disks
    st, b = api(node, "POST", "/vms", body)
    check(st in (200, 201), f"create {vm} on {NAME[node]} ({st} {b if st >= 300 else ''})")
    st, b = api(node, "POST", f"/vms/{vm}/import-disk", {"source": "/var/lib/hx/images/cirros.img"})
    check(st in (200, 201), f"{vm}: CirrOS imported as disk0 ({st} {b if st >= 300 else ''})")
    if start:
        st, b = api(node, "POST", f"/vms/{vm}/start")
        check(st == 200, f"{vm} started ({st} {b if st != 200 else ''})")
    st, b = api(node, "GET", f"/vms/{vm}")
    return b


def migrate(node, vm, target, mtype, bandwidth=None, wait=True):
    body = {"resource_type": "vm", "resource_id": vm, "target_node": target, "migration_type": mtype}
    if bandwidth:
        body["bandwidth_limit"] = bandwidth
    st, job = api(node, "POST", "/migration/jobs", body)
    check(st == 201, f"{mtype} migration of {vm} to {target} started ({st} {job if st != 201 else ''})")
    if not wait:
        return job
    return wait_job(node, job["job_id"])


def wait_job(node, job_id, timeout=1500):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        st, jobs = api(node, "GET", "/migration/jobs")
        j = next((x for x in jobs if x["job_id"] == job_id), None)
        if j and (j["status"], j["progress"]) != last:
            last = (j["status"], j["progress"])
            log(f"    job {job_id}: {j['status']} {j['progress']}% {j.get('error') or ''}")
        if j and j["status"] in ("completed", "failed", "cancelled"):
            return j
        time.sleep(2)
    raise SystemExit(f"job {job_id} did not finish")


def qemu_pid(node, vm):
    return sh(node, f"cat /var/run/horcrux-vm-{vm}.pid 2>/dev/null; true")


def gone_from(node, vm):
    st, _ = api(node, "GET", f"/vms/{vm}")
    files = sh(node, f"ls {STORE}/{vm}* 2>/dev/null; true")
    # [h]: the pattern must not match this pgrep's own shell
    procs = sh(node, f"pgrep -f '[h]orcrux-vm-{vm}.pid' || true")
    return st == 404 and files == "" and procs == "", (st, files, procs)


# --- cluster membership ----------------------------------------------------
log("== cluster: each node knows the other")
for a, b in ((N1, N2), (N2, N1)):
    api(a, "DELETE", f"/cluster/nodes/{NAME[b]}")
    st, body = api(a, "POST", f"/cluster/nodes/{NAME[b]}", {"ip": IP[b]})
    check(st == 201, f"{NAME[a]} registers {NAME[b]} at {IP[b]} ({st} {body})")

# --- 1. live migration of a UEFI + Q35 + TPM VM with two disks -------------
log("== 1. live migration: UEFI/Q35/TPM, VirtIO + SCSI SSD disks, local storage")
st, opts = api(N1, "GET", "/vms/host-options")
check(opts["ovmf"] and opts["swtpm"], f"nodes have OVMF + swtpm ({opts['ovmf']}, {opts['swtpm']})")
vm = "mig-uefi"
cfg = create_vm(
    N1,
    vm,
    hardware={"machine": "q35", "firmware": "ovmf", "tpm": True, "cpu_type": "qemu64"},
    disks=[
        {"path": "", "size_gb": 1, "disk_type": "virtio", "cache": ""},
        {"path": "", "size_gb": 1, "disk_type": "scsi", "cache": "none", "ssd": True, "discard": True},
    ],
)
disk0 = cfg["disks"][0]["path"]
check(len(cfg["disks"]) == 2 and cfg["disks"][1]["disk_type"] == "scsi", "two disks recorded")
args = sh(N1, f"tr '\\0' ' ' < /proc/$(cat /var/run/horcrux-vm-{vm}.pid)/cmdline")
for want in ["q35", "if=pflash", "tpm-tis", "virtio-blk-pci", "scsi-hd", "rotation_rate=1", "-cpu qemu64"]:
    check(want in args, f"QEMU runs with {want}")
check("UEFI" in guest(N1, disk0, "test -d /sys/firmware/efi && echo UEFI"), "guest booted with UEFI")
check(guest(N1, disk0, "cat /sys/block/sda/queue/rotational").strip().endswith("0"), "SCSI disk is an SSD in the guest")
tpm = qmp(N1, disk0, "query-tpm")
check(any(t.get("model") == "tpm-tis" for t in tpm.get("return", [])), f"TPM 2.0 attached ({tpm})")
marker = f"hx-live-{int(time.time())}"
guest(N1, disk0, f"echo {marker} > /home/cirros/m; sync")
up_before = uptime(N1, disk0)
st, _ = api(N1, "POST", f"/vms/{vm}/firewall/rules",
            {"id": "mig-fw", "enabled": True, "action": "DROP", "direction": "out",
             "protocol": "udp", "dport": "9999", "log": False, "position": 0})
check(st == 201, "VM firewall rule added on node1")
job = migrate(N1, vm, "node2", "live")
check(job["status"] == "completed", f"live migration completed ({job.get('error')})")
st, moved = api(N2, "GET", f"/vms/{vm}")
check(st == 200 and moved["status"] == "running", f"{vm} runs on node2 ({st} {moved})")
d0 = moved["disks"][0]["path"]
check(d0.startswith(STORE) and len(moved["disks"]) == 2, f"disks on node2's storage ({d0})")
ok, why = gone_from(N1, vm)
check(ok, f"nothing of {vm} left on node1 {why}")
got = guest(N2, d0, "cat /home/cirros/m")
check(marker in got, f"file written before the move is there ({got!r})")
up_after = uptime(N2, d0)
check(up_after > up_before, f"guest kept running: uptime {up_before} -> {up_after} (no reboot)")
check("UEFI" in guest(N2, d0, "test -d /sys/firmware/efi && echo UEFI"), "still UEFI on node2")
tpm = qmp(N2, d0, "query-tpm")
check(any(t.get("model") == "tpm-tis" for t in tpm.get("return", [])), "TPM came along")
check(sh(N2, f"test -f {d0.rsplit('.', 1)[0]}.efivars.qcow2 && echo y") == "y", "EFI variable store on node2")
st, rules = api(N2, "GET", f"/vms/{vm}/firewall/rules")
check(st == 200 and any(r["id"] == "mig-fw" for r in rules), "firewall rule moved to node2")
check("ct:" not in sh(N2, "nft list chain bridge horcrux forward") and f"vm:{vm}" in sh(N2, "nft list chain bridge horcrux forward"),
      "rule installed in node2's nftables")
check(f"vm:{vm}" not in sh(N1, "nft list chain bridge horcrux forward 2>/dev/null; true"), "and removed from node1's")

# --- 2. offline migration back of the running VM ---------------------------
log("== 2. offline migration back (running VM: shut down, copied, started)")
job = migrate(N2, vm, "node1", "offline")
check(job["status"] == "completed", f"offline migration completed ({job.get('error')})")
st, back = api(N1, "GET", f"/vms/{vm}")
check(st == 200 and back["status"] == "running", f"{vm} runs on node1 again ({st})")
ok, why = gone_from(N2, vm)
check(ok, f"nothing of {vm} left on node2 {why}")
d0 = back["disks"][0]["path"]
check(marker in guest(N1, d0, "cat /home/cirros/m"), "file survived the offline copy")
src_log = sh(N2, "cat /var/lib/hx/api.log")
check("sha256 verified" in src_log, "copies were SHA-256 verified")
check(sh(N1, f"test -d {d0.rsplit('.', 1)[0]}.tpm && ls {d0.rsplit('.', 1)[0]}.tpm") != "", "TPM state copied")
api(N1, "POST", f"/vms/{vm}/stop")

# --- 3. cancel -------------------------------------------------------------
log("== 3. cancel a slow live migration")
vm = "mig-cancel"
cfg = create_vm(N1, vm)
d0 = cfg["disks"][0]["path"]
guest(N1, d0, "true")  # booted
pid = qemu_pid(N1, vm)
job = migrate(N1, vm, "node2", "live", bandwidth=1, wait=False)
for _ in range(60):
    st, jobs = api(N1, "GET", "/migration/jobs")
    j = next(x for x in jobs if x["job_id"] == job["job_id"])
    if j["status"] == "running" and j["progress"] > 6:
        break
    time.sleep(1)
check(j["status"] == "running", f"migration is transferring ({j['status']} {j['progress']}%)")
check(sh(N2, f"pgrep -f '[h]orcrux-vm-{vm}.pid' || true") != "", "target QEMU was waiting for the VM")
st, _ = api(N1, "DELETE", f"/migration/jobs/{job['job_id']}")
check(st == 204, "cancel accepted")
time.sleep(8)
j = wait_job(N1, job["job_id"], timeout=60)
check(j["status"] == "cancelled", f"job stays cancelled ({j['status']})")
st, here = api(N1, "GET", f"/vms/{vm}")
check(here["status"] == "running" and qemu_pid(N1, vm) == pid, "VM still running on node1 in the same QEMU")
check(qmp(N1, d0, "query-status")["return"]["status"] == "running", "guest not paused")
check(qmp(N1, d0, "query-block-jobs")["return"] == [], "disk mirror stopped")
ok, why = gone_from(N2, vm)
check(ok, f"no trace of {vm} on node2 {why}")
api(N1, "POST", f"/vms/{vm}/stop")
api(N1, "DELETE", f"/vms/{vm}")

# --- 4. target can't run the VM --------------------------------------------
log("== 4. refused by the target (bridge only on node1)")
st, b = api(N1, "POST", "/network/bridges", {"name": "hxonly1"})
check(st in (200, 201), f"bridge hxonly1 on node1 ({st} {b})")
vm = "mig-refused"
create_vm(N1, vm, hardware={"nets": [{"bridge": "hxonly1"}]}, start=False)
job = migrate(N1, vm, "node2", "offline")
check(job["status"] == "failed" and "hxonly1" in (job.get("error") or ""), f"failed with the reason ({job.get('error')})")
st, here = api(N1, "GET", f"/vms/{vm}")
check(st == 200 and sh(N1, f"test -f {here['disks'][0]['path']} && echo y") == "y", "VM and disk stay on node1")
ok, why = gone_from(N2, vm)
check(ok, f"nothing on node2 {why}")
api(N1, "DELETE", f"/vms/{vm}")

# --- 5. shared storage -----------------------------------------------------
log("== 5. live migration on shared storage (nothing copied)")
for n in (N1, N2):
    sh(n, "bash /hx/start-api.sh /shared/vms")
vm = "mig-shared"
cfg = create_vm(N1, vm)
d0 = cfg["disks"][0]["path"]
check(d0.startswith("/shared/vms/"), f"disk on shared storage ({d0})")
marker = f"hx-shared-{int(time.time())}"
guest(N1, d0, f"echo {marker} > /home/cirros/m; sync")
up_before = uptime(N1, d0)
job = migrate(N1, vm, "node2", "live")
check(job["status"] == "completed", f"completed ({job.get('error')})")
st, moved = api(N2, "GET", f"/vms/{vm}")
check(st == 200 and moved["disks"][0]["path"] == d0, "node2 uses the same disk file")
# The source QEMU's exit must not take the target's console sockets with it.
check(qmp(N2, d0, "query-status").get("return", {}).get("status") == "running", "node2's QMP socket answers")
check(f"Migration {job['job_id']}: 1 of 1 file(s) on storage shared" in sh(N1, "cat /var/lib/hx/api.log"),
      "the disk was detected as shared and not copied")
check(sh(N2, f"test -f {d0} && echo y") == "y", "the shared disk was not deleted by the source")
check(marker in guest(N2, d0, "cat /home/cirros/m") and uptime(N2, d0) > up_before, "guest kept running with its data")
api(N2, "POST", f"/vms/{vm}/stop")
api(N2, "DELETE", f"/vms/{vm}")

log("vm_migrate_e2e: all checks passed")
