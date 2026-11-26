"""Installed Horcrux node + Ceph RBD VM disks.

Installs the ISO unattended, boots the installed disk, then checks what a VM disk on a
Ceph RBD pool needs from the node and walks the vm-disk-on-rbd-pool step:

  1. the node has the `rbd` client and QEMU's rbd block driver;
  2. with no Ceph config on the node, a Ceph pool cannot be added (refused, nothing created);
  3. with `--arg ceph_conf=/path/ceph.conf` (a cluster reachable from the guest, e.g. one
     started by horcrux-api/tests/ceph_cluster_boot.sh on the host) the node adds the pool,
     `POST /api/vms/<id>/disks {size_gb, storage: <ceph pool id>}` creates the RBD image,
     and removing the VM removes it.  Step 3 is noted as skipped without `ceph_conf`.

    python3 -m vmtest run scenarios/ceph_vm_disk_rbd.py --arg iso=/path/x.iso [--arg ceph_conf=...] [--arg rados_pool=hxpool]
"""
import json
import os
import shutil
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PROJECTS, make_answers, unattended_install  # noqa: E402


def define(args):
    P = PROJECTS["horcrux"]
    answers, key = make_answers("horcrux")
    node = NodeSpec(name="horcrux", mem=int(args.get("mem", 4096)), cpus=2, iso=args["iso"], iso_media="usb", firmware="uefi",
                    disks=[Disk("24G")], extra_drives=[(answers, "usb")],
                    nics=[Nic("user", forwards={"ssh": 22, "web": P["port"]})], ssh_key=key, console="none", boot_timeout=420)

    def run(m):
        n = m["horcrux"]
        m.phase("install")
        unattended_install(m, n)
        n.start()
        n.wait_for_ssh(420)
        n.wait_for_unit(P["svc"], 300)
        n.wait_until(lambda: n.http("web", P["health"])[0] == 200, 300, 5, "web health 200")

        m.phase("ceph client on the installed node")
        n.succeed("command -v rbd")
        m.check("rbd-client-installed", True, "rbd on PATH")
        out = n.succeed("qemu-img --help | grep -i 'supported formats'")
        m.check("qemu-rbd-driver", "rbd" in out, out.strip()[:200])

        pw = args.get("password", "")
        sc, body = n.http("web", "/api/auth/login", "POST", {"username": "administrator", "password": pw})
        tok = json.loads(body).get("ticket") if sc == 200 else None

        def api(method, path, payload=None):
            hdr = {"Authorization": "Bearer " + tok} if tok else None
            sc, b = n.http("web", "/api" + path, method, payload, headers=hdr)
            try:
                return sc, json.loads(b)
            except ValueError:
                return sc, b

        m.phase("vm-disk-on-rbd-pool")
        conf = args.get("ceph_conf")
        if not conf:
            m.note("vm-disk-on-rbd-pool", "no --arg ceph_conf: no cluster to talk to, real-cluster step skipped "
                   "(covered by horcrux-api/tests/ceph_storage_e2e.sh)")
        elif tok:
            rados = args.get("rados_pool", "hxpool")
            vm = "rbdvm1"
            sc, _ = api("POST", "/vms", {"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1,
                                         "memory": 256, "disk_size": 1, "status": "stopped", "display": {"protocol": "none"},
                                         "hardware": {}})
            m.check("create-vm", sc in (200, 201), str(sc))
            sc, b = api("POST", "/storage/pools", {"name": "rbdpool", "storage_type": "ceph", "path": rados})
            m.check("ceph-pool-refused-without-client-config", sc >= 400, f"{sc} {str(b)[:160]}")
            n.put_file(conf, "/etc/ceph/ceph.conf")
            sc, b = api("POST", "/storage/pools", {"name": "rbdpool", "storage_type": "ceph", "path": rados})
            m.check("ceph-pool-added", sc in (200, 201), f"{sc} {str(b)[:160]}")
            pid = b["id"]
            sc, b = api("POST", f"/vms/{vm}/disks", {"size_gb": 1, "storage": pid})
            m.check("vm-disk-on-rbd-pool", sc == 201, f"{sc} {str(b)[:160]}")
            info = n.succeed(f"rbd info --format json {rados}/vm-{vm}-disk-1")
            m.check("rbd-image-exists", json.loads(info)["size"] == 1024 ** 3, info[:120])
            api("DELETE", f"/vms/{vm}")
            n.fail(f"rbd info {rados}/vm-{vm}-disk-1")
            m.check("rbd-image-removed-with-vm", True, "")
        else:
            m.note("vm-disk-on-rbd-pool", "could not log in (pass --arg password=...)")
        n.power_off()
        shutil.rmtree(os.path.dirname(key), ignore_errors=True)

    return Test("ceph-vm-disk-rbd", nodes=[node], run=run, timeout=3600,
                description="installed node: Ceph client present, VM disk on an RBD pool")
