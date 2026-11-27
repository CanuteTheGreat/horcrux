"""SAN multipath on two installed Horcrux nodes (iSCSI + NVMe/TCP + shared LVM + VM disks on the LUN), OpenRC and systemd ISOs.

 san  = the array: Debian with LIO (targetcli) on 4 portals = 2 NICs x 2 addresses, nvmet over TCP on 2 addresses (one per NIC)
        sana hub: 10.77.1.1 + 10.77.1.2    sanb hub: 10.77.2.1 + 10.77.2.2
 hx1, hx2 = Horcrux installed from the ISO (unattended installer), cluster LAN hub `clan` 10.77.0.11/.12, SAN NICs on sana/sanb

Proves on real installed nodes (their own init system, packages, firewall and kernel):
 - the SAN target and the NVMe target are created ONCE on hx1 and reach hx2 through the cluster (peer sync), both nodes log in
   to all 4 iSCSI portals (one dm-multipath map, 4 active paths per LUN) and connect both NVMe paths (native multipath)
 - a shared VG (lvmlockd + sanlock, own host id per node) on the multipath LUN, created on hx1, is a storage pool on both nodes
 - a VM's disk on that pool is a volume `vm-<id>-disk-0` (CirrOS imported into it), the VM runs on hx1 and writes to its disk
 - the array's second NIC is unplugged: the guest keeps writing, both nodes' path view shows the failed paths (2 iSCSI + 1 NVMe),
   the cluster view of hx1 shows hx2's paths too; the NIC comes back and every path recovers
 - snapshots as a qcow2 volume chain on a second LUN: snapshot, write, rollback, the data of the snapshot is back
 - deleting the VM deletes its volume with discard on: the array's backing file shrinks (UNMAP reached the array)

    python3 -m vmtest run scenarios/san_multipath.py --arg iso=/iso/horcrux-server-openrc-daily-x86_64.iso \\
        --arg target=/w/san-target.qcow2 --arg cirros=/w/cirros.img --arg repo=/repo [--arg bin=/w/horcrux-api] \\
        [--arg tools=emerge] --out DIR

`target` comes from horcrux-api/tests/san_target_image.sh (root password = TGT_PW below). `bin` replaces the installed manager
with a build under test. `tools=emerge` installs the SAN host tools (USE=san) from the binhost/source on an image that lacks them.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Network, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

TGT_PW = "Tg7#san-array-41Lq"
IQN = "iqn.2003-01.org.linux-iscsi.hxsan:sn.vmtest"
NQN = "nqn.2014-08.org.horcrux:hxsan-vmtest"
HTTP_PORT = 18097
NODES = {"hx1": 11, "hx2": 12}


def define(args):
    iso, target, cirros, repo = args["iso"], args["target"], args["cirros"], args["repo"]
    binary, tools = args.get("bin"), args.get("tools", "")
    mem = int(args.get("mem", 4096))
    nodes = []
    for name, last in NODES.items():
        answers, key = make_answers("horcrux", hostname=f"{name}.vmt.test")
        nodes.append(NodeSpec(name=name, mem=mem, cpus=2, iso=iso, iso_media="usb", firmware="uefi", disks=[Disk("24G")],
                              extra_drives=[(answers, "usb")], ssh_key=key, console="none", boot_timeout=600, cpu="host",
                              nics=[Nic("user", forwards={"ssh": 22, "web": 8006}), Nic("clan", mac=f"52:54:00:0c:00:{last:02x}"),
                                    Nic("sana", mac=f"52:54:00:0a:00:{last:02x}"), Nic("sanb", mac=f"52:54:00:0b:00:{last:02x}")]))
    san = NodeSpec(name="san", mem=1536, cpus=2, image=target, disks=[Disk("16G")], login_password=TGT_PW, boot_timeout=300,
                   nics=[Nic("user", forwards={}), Nic("sana", mac="52:54:00:0a:00:01"), Nic("sanb", mac="52:54:00:0b:00:01")])

    def run(m):
        S = m.san
        hxs = [getattr(m, n) for n in NODES]

        m.phase("install both Horcrux nodes, boot the array")
        errs = []

        def inst(n):
            try:
                unattended_install(m, n)
            except Exception as e:          # noqa: BLE001 - reported below
                errs.append(f"{n.name}: {e}")
        ts = [threading.Thread(target=inst, args=(n,)) for n in hxs]
        for t in ts:
            t.start()
        S.start()
        S.wait_for_shell()
        for t in ts:
            t.join()
        m.check("both-nodes-installed", not errs, "; ".join(errs))

        m.phase("the array: LIO on 4 portals, nvmet on 2, LUNs with UNMAP")
        ifn = lambda node, mac: node.succeed(f"ip -o link | grep -i {mac} | cut -d: -f2 | tr -d ' '").strip()   # noqa: E731
        a, b = ifn(S, "52:54:00:0a:00:01"), ifn(S, "52:54:00:0b:00:01")
        S.succeed(f"ip link set {a} up; ip link set {b} up; ip addr add 10.77.1.1/24 dev {a}; ip addr add 10.77.1.2/24 dev {a}; "
                  f"ip addr add 10.77.2.1/24 dev {b}; ip addr add 10.77.2.2/24 dev {b}")
        S.succeed("modprobe target_core_mod; modprobe iscsi_target_mod; modprobe nvmet; modprobe nvmet_tcp; mkdir -p /var/lib/san; "
                  "targetcli clearconfig confirm=True >/dev/null 2>&1; true", timeout=120)
        tc = [f"/backstores/fileio create name=lun0 file_or_dev=/var/lib/san/lun0.img size=6G sparse=true",
              "/backstores/fileio/lun0 set attribute emulate_tpu=1",
              f"/backstores/fileio create name=lun1 file_or_dev=/var/lib/san/lun1.img size=3G sparse=true",
              "/backstores/fileio/lun1 set attribute emulate_tpu=1",
              f"/iscsi create {IQN}", f"/iscsi/{IQN}/tpg1/portals delete 0.0.0.0 3260"]
        tc += [f"/iscsi/{IQN}/tpg1/portals create {ip} 3260" for ip in ("10.77.1.1", "10.77.1.2", "10.77.2.1", "10.77.2.2")]
        tc += [f"/iscsi/{IQN}/tpg1/luns create /backstores/fileio/lun0", f"/iscsi/{IQN}/tpg1/luns create /backstores/fileio/lun1",
               f"/iscsi/{IQN}/tpg1 set attribute generate_node_acls=1 demo_mode_write_protect=0 authentication=0 cache_dynamic_acls=1"]
        for c in tc:
            S.execute(f"targetcli {c} >/dev/null 2>&1", timeout=120)
        m.check("array-4-portals-2-luns", S.succeed(f"targetcli ls /iscsi/{IQN}/tpg1 | grep -c -E '3260|lun[01] '", timeout=60).strip() >= "6",
                S.succeed(f"targetcli ls /iscsi/{IQN}/tpg1", timeout=60)[-800:])
        C = "/sys/kernel/config/nvmet"
        S.succeed(f"truncate -s 1G /var/lib/san/nvme0.img; L=$(losetup -f --show /var/lib/san/nvme0.img); "
                  f"mkdir -p {C}/subsystems/{NQN}/namespaces/1; echo 1 > {C}/subsystems/{NQN}/attr_allow_any_host; "
                  f"echo $L > {C}/subsystems/{NQN}/namespaces/1/device_path; echo 1 > {C}/subsystems/{NQN}/namespaces/1/enable; "
                  + "; ".join(f"mkdir -p {C}/ports/{i}; echo tcp > {C}/ports/{i}/addr_trtype; echo ipv4 > {C}/ports/{i}/addr_adrfam; "
                              f"echo {ip} > {C}/ports/{i}/addr_traddr; echo 4420 > {C}/ports/{i}/addr_trsvcid; "
                              f"ln -s {C}/subsystems/{NQN} {C}/ports/{i}/subsystems/{NQN}" for i, ip in ((1, "10.77.1.1"), (2, "10.77.2.1"))))
        m.check("array-nvmet-2-ports", S.succeed(f"ls {C}/ports/*/subsystems | grep -c {NQN}").strip() == "2", "")

        m.phase("first boot of both nodes, cluster LAN + SAN addresses, the manager under test")
        env = f"HX_USER=hxadmin HX_PASS='{PW}' "
        d = tempfile.mkdtemp(prefix="hxsan-")
        for src, name in ([(binary, "horcrux-api")] if binary else []) + [(cirros, "cirros.img"),
                                                                         (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py")]:
            try:
                os.link(src, os.path.join(d, name))
            except OSError:
                shutil.copy(src, os.path.join(d, name))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        base = f"http://10.0.2.2:{HTTP_PORT}"

        def hx(n, method, path, body=None, ok=True, timeout=300):
            c = f"{env}python3 /root/hx_node.py api {method} {path}" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=timeout).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                raise AssertionError(f"{n.name} {method} {path}: {r}")
            return r

        try:
            for n in hxs:
                last = NODES[n.name]
                n.start()
                n.wait_for_ssh(600)
                n.wait_for_unit("horcrux-api", 300)
                n.wait_until_succeeds("! pgrep -f '[h]orcrux-firstboot'", 900, 10)
                for mac, ip in ((f"52:54:00:0c:00:{last:02x}", f"10.77.0.{last}"), (f"52:54:00:0a:00:{last:02x}", f"10.77.1.{last}"),
                                (f"52:54:00:0b:00:{last:02x}", f"10.77.2.{last}")):
                    i = ifn(n, mac)
                    n.succeed(f"ip link set {i} up; ip addr replace {ip}/24 dev {i}")
                n.succeed(f"curl -sf --retry 3 {base}/hx_node.py -o /root/hx_node.py && mkdir -p /var/lib/horcrux/images && "
                          f"curl -sf --retry 3 {base}/cirros.img -o /var/lib/horcrux/images/cirros.img", timeout=600)
                if binary:
                    n.succeed(f"curl -sf --retry 3 {base}/horcrux-api -o /tmp/horcrux-api.new && chmod 755 /tmp/horcrux-api.new", timeout=600)
                    n.stop_unit("horcrux-api")
                    n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
                    n.start_unit("horcrux-api")
                    n.wait_for_unit("horcrux-api", 120)
                n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")
                m.check(f"{n.name}:init", n.init.name in ("openrc", "systemd"), n.init.name)
                m.check(f"{n.name}:reaches-all-4-portals", all(n.execute(f"bash -c ': </dev/tcp/{ip}/3260'", 20)[0] == 0
                                                               for ip in ("10.77.1.1", "10.77.1.2", "10.77.2.1", "10.77.2.2")), "")
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)

        m.phase("SAN host tools on the installed image")
        for n in hxs:
            have = n.execute("command -v iscsiadm && command -v multipathd && command -v sanlock && command -v lvmlockd && command -v nvme", 30)[0] == 0
            if not have and tools == "emerge":
                n.succeed("mkdir -p /etc/portage/package.use && printf '%s\\n' 'app-emulation/horcrux-bin san' 'sys-fs/lvm2 lvm sanlock' "
                          "> /etc/portage/package.use/hx-san-vmtest")
                n.succeed("emerge --sync >/dev/null 2>&1 || emerge-webrsync >/dev/null 2>&1; true", timeout=3600)
                r = n.execute("emerge -q --getbinpkg --usepkg -1 --changed-use sys-block/open-iscsi sys-fs/multipath-tools "
                              "sys-cluster/sanlock sys-apps/nvme-cli sys-fs/lvm2 > /root/san-emerge.log 2>&1; echo rc=$?; tail -30 /root/san-emerge.log",
                              timeout=5400)[1]
                m.note(f"{n.name}:emerged-san-tools", "rc=0" in r, r[-1500:])
                have = n.execute("command -v iscsiadm && command -v multipathd && command -v sanlock && command -v lvmlockd && command -v nvme", 30)[0] == 0
            st = hx(n, "GET", "/san/status")["body"]
            m.check(f"{n.name}:san-tools-present", have and all(st["tools"].get(t) for t in ("iscsiadm", "multipathd", "lvmlockd")),
                    json.dumps(st.get("tools"))[:600])

        h1, h2 = hxs
        m.phase("one cluster: create on hx1, join hx2")
        hx(h1, "POST", "/cluster/create", {"name": "hxsan", "address": "10.77.0.11"})
        r = hx(h2, "POST", "/cluster/join", {"peer": "10.77.0.11", "username": "hxadmin", "password": PW, "address": "10.77.0.12"}, ok=False)
        m.check("hx2-joined", r["status"] < 300, json.dumps(r)[:600])
        h1.wait_until(lambda: len(hx(h1, "GET", "/cluster/nodes", ok=False)["body"] or []) >= 2, 180, 5, "2 members")

        m.phase("the SAN created once on hx1 reaches hx2; every path on both nodes")
        r = hx(h1, "POST", "/san/discover", {"portals": ["10.77.1.1"]})
        m.check("discover-finds-4-portals", len(r["body"]["targets"][0]["portals"]) == 4, json.dumps(r["body"])[:600])
        hx(h1, "PUT", "/san/settings", {"sanlock_watchdog": False, "reconcile_interval_secs": 15})
        hx(h1, "POST", "/san/targets", {"id": "arr", "iqn": IQN, "portals": ["10.77.1.1"]})
        hx(h1, "POST", "/san/nvme/targets", {"id": "nv", "subnqn": NQN, "addresses": ["10.77.1.1:4420", "10.77.2.1:4420"]})

        def luns(n):
            return hx(n, "GET", "/san/paths")["body"]["luns"]

        def iscsi_view(n):
            return sorted((l["active_paths"], len(l["paths"])) for l in luns(n) if l.get("map") and l.get("target") == IQN)

        def nvme_view(n):
            return [(l["active_paths"], len(l["paths"])) for l in luns(n) if l.get("target") == NQN]

        for n in hxs:
            hx(n, "POST", "/san/apply", timeout=600)
            n.wait_until(lambda: iscsi_view(n) == [(4, 4), (4, 4)] and nvme_view(n) == [(2, 2)], 300, 10, f"{n.name} all paths")
            m.check(f"{n.name}:2-luns-x-4-iscsi-paths", iscsi_view(n) == [(4, 4), (4, 4)], json.dumps(luns(n))[:1500])
            m.check(f"{n.name}:nvme-2-paths", nvme_view(n) == [(2, 2)], json.dumps(nvme_view(n)))
        m.check("hx2-got-the-target-by-peer-sync", any(t["id"] == "arr" for t in hx(h2, "GET", "/san/targets")["body"]), "")
        by_lun = {l["lun"]: l["wwid"] for l in luns(h1) if l.get("map") and l.get("target") == IQN}
        m.check("lun-0-and-1-mapped", sorted(by_lun) == [0, 1], json.dumps(by_lun))
        lun0, lun1 = by_lun[0], by_lun[1]

        m.phase("shared LVM on the LUN: one VG, a pool on both nodes")
        hx(h1, "POST", "/san/lvm", {"id": "sanlvm", "vg_name": "vg_hxsan", "device": lun0, "discard_removed": True, "create_vg": True}, timeout=600)
        hx(h1, "POST", "/san/lvm", {"id": "sanchain", "vg_name": "vg_hxchain", "device": lun1, "create_vg": True,
                                    "snapshot_as_volume_chain": True}, timeout=600)
        for n in hxs:
            n.wait_until(lambda: "sanlock" in n.execute("vgs --noheadings -o vg_lock_type vg_hxsan 2>/dev/null", 30)[1], 300, 10, f"{n.name} vg")
            hx(n, "POST", "/san/apply", timeout=600)
            n.wait_until(lambda: n.execute("lvmlockctl -i 2>/dev/null | grep -q vg_hxsan", 30)[0] == 0, 300, 10, f"{n.name} lockspace")
            m.check(f"{n.name}:lockspace-started", True, "")
        ids = {n.name: n.succeed("sed -n 's/^\\s*host_id = //p' /etc/lvm/lvmlocal.conf").strip() for n in hxs}
        m.check("own-sanlock-host-id-per-node", len(set(ids.values())) == 2 and all(ids.values()), json.dumps(ids))

        def pool_id(n, name):
            return [p["id"] for p in hx(n, "GET", "/storage/pools")["body"] if p["name"] == name][0]

        P = pool_id(h1, "sanlvm")
        m.check("hx2-has-the-pool", bool(pool_id(h2, "sanlvm")), "")

        m.phase("a VM whose disk is a volume of the shared VG")
        hx(h1, "POST", "/vms", {"id": "301", "name": "sanvm", "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 256,
                                "disk_size": 1, "status": "stopped", "display": {"protocol": "none"}, "hardware": {},
                                "disks": [{"path": "", "size_gb": 1, "disk_type": "virtio", "cache": "none", "storage": P}]})
        cfg = hx(h1, "GET", "/vms/301")["body"]
        m.check("vm-disk-is-vm-301-disk-0", cfg["disks"][0]["path"] == "/dev/vg_hxsan/vm-301-disk-0" and cfg["disks"][0].get("format") == "raw",
                json.dumps(cfg["disks"]))
        hx(h1, "POST", "/vms/301/import-disk", {"source": "/var/lib/horcrux/images/cirros.img"}, timeout=900)
        hx(h1, "POST", "/vms/301/start")
        sock = h1.succeed("ls /var/lib/horcrux/vms/blk/*vm-301-disk-0.serial.sock").strip()

        def guest(cmd):
            out = h1.succeed(f"{env}python3 /root/hx_node.py serial {sock} \"{cmd}\"", timeout=900).strip().splitlines()
            return out[-1] if out else ""

        def gnum(cmd, tries=10):
            for _ in range(tries):
                try:
                    return int(guest(cmd).split()[-1])
                except (IndexError, ValueError):
                    time.sleep(5)
            raise AssertionError(f"no number from the guest for {cmd!r}")

        guest("nohup sh -c 'i=0; while :; do dd if=/dev/urandom of=/root/io bs=4k count=64 conv=fsync 2>/dev/null && i=\\$((i+1)); "
              "echo \\$i > /dev/shm/cnt; done' >/dev/null 2>&1 &")
        time.sleep(15)
        c0 = gnum("cat /dev/shm/cnt")
        m.check("guest-writes-to-its-lv", c0 > 0, str(c0))

        m.phase("the array's second NIC is unplugged")
        S.set_link(2, False)

        def failed(n):
            return sorted((l.get("failed_paths", 0)) for l in luns(n) if l.get("map") or l.get("target") == NQN)

        for n in hxs:
            n.wait_until(lambda: failed(n) == [1, 2, 2], 240, 6, f"{n.name} failed paths")
            m.check(f"{n.name}:path-view-shows-the-dead-nic", failed(n) == [1, 2, 2], json.dumps(luns(n))[:1500])
        dead = sorted(x["portal"] for l in luns(h1) if l.get("target") == IQN and l.get("map") for x in l["paths"] if x["state"] == "failed")
        m.check("failed-iscsi-paths-are-10.77.2.x", set(dead) == {"10.77.2.1:3260", "10.77.2.2:3260"}, json.dumps(dead))
        c1 = gnum("cat /dev/shm/cnt")
        time.sleep(20)
        c2 = gnum("cat /dev/shm/cnt")
        m.check("guest-io-continues-on-the-other-nic", c2 > c1 > c0, f"{c0} -> {c1} -> {c2}")
        other = [x["name"] for x in hx(h1, "GET", "/cluster/nodes")["body"] if x["name"].startswith("hx2")]
        cv = hx(h1, "GET", f"/san/nodes/{other[0]}/paths", ok=False) if other else {"status": 0, "body": "hx2 not a member"}
        cvf = sorted(l.get("failed_paths", 0) for l in (cv["body"].get("luns", []) if isinstance(cv["body"], dict) else [])
                     if l.get("map") or l.get("target") == NQN)
        m.check("hx1-shows-hx2-failed-paths", cv["status"] == 200 and cvf == [1, 2, 2], json.dumps(cv)[:800])
        S.set_link(2, True)
        for n in hxs:
            n.wait_until(lambda: iscsi_view(n) == [(4, 4), (4, 4)] and nvme_view(n) == [(2, 2)], 300, 10, f"{n.name} recover")
            m.check(f"{n.name}:every-path-recovers", True, "")
        c3 = gnum("cat /dev/shm/cnt")
        m.check("guest-never-stopped", c3 > c2 and "I/O error" not in h1.execute("dmesg | tail -50", 30)[1], f"{c2} -> {c3}")

        m.phase("snapshots as a volume chain on the second LUN")
        CP = pool_id(h1, "sanchain")
        hx(h1, "POST", f"/storage/pools/{CP}/volumes", {"name": "chainvol", "size": 1})
        V = "/dev/vg_hxchain/chainvol"
        qio = lambda c: h1.execute(f"qemu-io -f qcow2 -c '{c}' {V} 2>&1", 120)[1]   # noqa: E731
        qio("write -P 0xaa 0 4M")
        hx(h1, "POST", f"/storage/pools/{CP}/volumes/chainvol/snapshots", {"name": "s1"})
        qio("write -P 0xbb 0 4M")
        m.check("current-has-new-data", "verification failed" not in qio("read -P 0xbb 0 4M"), "")
        hx(h1, "POST", f"/storage/pools/{CP}/volumes/chainvol/snapshots/s1/rollback")
        out = qio("read -P 0xaa 0 4M")
        m.check("rollback-brings-the-snapshot-back", "verification failed" not in out and "read 4194304" in out, out[-300:])
        hx(h1, "DELETE", f"/storage/pools/{CP}/volumes/chainvol")

        m.phase("delete the VM: its volume goes, with discard, and the array gets the space back")
        hx(h1, "POST", "/vms/301/stop", {"force": True})
        h1.wait_until(lambda: hx(h1, "GET", "/vms/301")["body"]["status"] == "stopped", 180, 3, "stopped")
        S.succeed("sync")
        before = int(S.succeed("du -k /var/lib/san/lun0.img | cut -f1").strip())
        hx(h1, "DELETE", "/vms/301")
        m.check("vm-volume-deleted", h1.execute("lvs vg_hxsan/vm-301-disk-0", 30)[0] != 0, h1.execute("lvs vg_hxsan", 30)[1])
        S.succeed("sync")
        after = int(S.succeed("du -k /var/lib/san/lun0.img | cut -f1").strip())
        m.check("array-got-the-space-back", after < before - 50000, f"{before}K -> {after}K")
        m.check("no-500-in-the-api-log", " 500 " not in h1.unit_log("horcrux-api", 400), "")

    return Test(name="san_multipath", nodes=nodes + [san], run=run, timeout=int(args.get("timeout", 10800)),
                networks=[Network("clan"), Network("sana"), Network("sanb")])
