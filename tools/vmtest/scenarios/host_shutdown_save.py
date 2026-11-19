"""Host reboot with a running VM: the VM is SAVED to encrypted disk and RESUMED where it was (Horcrux, OpenRC and systemd).

A real Horcrux ISO is installed on an encrypted (xfs-luks) root, the Horcrux manager under test is dropped in (stop the unit, replace the
binary, install the repo's horcrux-guests unit / init script), a CirrOS VM with a counter process and a RAM marker runs inside it, and the
HOST (the vmtest node) is rebooted with its init system's own `reboot`.  Proves, with the guest's own counter, that memory is continuous
across the host reboot, that a VM with passthrough devices is shut down cleanly instead of saved (the reason is recorded), that saved
memory is refused on unencrypted storage (/boot, tmpfs) and that a save that exceeds its timeout falls back to a clean shutdown.

    python3 -m vmtest run scenarios/host_shutdown_save.py --arg iso=/iso/horcrux-server-openrc-daily-x86_64.iso \\
        --arg bin=/w/horcrux-api --arg cirros=/w/cirros.img --arg repo=/repo --out DIR

`bin` is a horcrux-api built for the guest (any recent Linux glibc), `repo` the horcrux checkout (units, hx_node.py).
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

PW_DISK = "Dq4#sB8-luks-63Nw"      # the disk passphrase (never equal to an account password)
LUKS_PROMPT = r"(?i)(enter passphrase|passphrase for|enter key|please enter (the )?(disk )?(passphrase|password))"
HTTP_PORT = 18099


def define(args):
    iso, binary, cirros, repo = args["iso"], args["bin"], args["cirros"], args["repo"]
    answers, key = make_answers("horcrux", fs="xfs-luks", luks=PW_DISK)
    node = NodeSpec(name="hx", mem=int(args.get("mem", 6144)), cpus=4, iso=iso, iso_media="usb", firmware="uefi",
                    disks=[Disk("32G")], extra_drives=[(answers, "usb")],
                    nics=[Nic("user", forwards={"ssh": 22, "web": 8006})], ssh_key=key, console="none", boot_timeout=600,
                    cpu="host")          # the inner VMs get KVM when the runner nests, TCG otherwise

    def run(m):
        n = m.hx
        m.phase("install (xfs-luks: the root is encrypted)")
        unattended_install(m, n)

        def unlock_boot(first=False):
            """Start (first) or let the rebooting host come up, typing the disk passphrase at the initramfs prompt."""
            end = time.time() + n.T(900)
            while time.time() < end:
                if n.q.expect(LUKS_PROMPT, 4):
                    n.q.send(PW_DISK + "\n")
                try:
                    n.wait_for_ssh(10)
                    return
                except Exception:
                    if not n.q.alive():
                        raise
            raise AssertionError("host did not come back")

        m.phase("first boot")
        n.start()
        n.ssh_ok = False
        unlock_boot()
        m.check("host-boots-and-ssh", True, "")
        n.wait_for_unit("horcrux-api", 300)
        init = n.init.name
        m.check("init-system", init in ("openrc", "systemd"), init)

        m.phase("drop in the manager under test")
        d = tempfile.mkdtemp(prefix="hxsrv-")
        for src, name in ((binary, "horcrux-api"), (cirros, "cirros.img"), (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py"),
                          (os.path.join(repo, "deploy/openrc/horcrux-guests"), "horcrux-guests.initd"),
                          (os.path.join(repo, "deploy/systemd/horcrux-guests.service"), "horcrux-guests.service")):
            try:
                os.link(src, os.path.join(d, name))
            except OSError:
                shutil.copy(src, os.path.join(d, name))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(1)
            base = f"http://10.0.2.2:{HTTP_PORT}"
            for f, dst, mode in (("horcrux-api", "/tmp/horcrux-api.new", "755"), ("cirros.img", "/root/cirros.img", "644"), ("hx_node.py", "/root/hx_node.py", "755"),
                                 ("horcrux-guests.initd", "/tmp/horcrux-guests.initd", "755"), ("horcrux-guests.service", "/tmp/horcrux-guests.service", "644")):
                n.succeed(f"curl -sf --retry 3 {base}/{f} -o {dst} && chmod {mode} {dst}", timeout=600)
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)
        n.succeed("pkill -x horcrux-api || true; true")
        n.stop_unit("horcrux-api")
        n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
        # the repo's init script (OpenRC) or unit (systemd): same unit name on both
        n.succeed("if [ -d /run/systemd/system ]; then install -m644 /tmp/horcrux-guests.service /etc/systemd/system/horcrux-guests.service; "
                  "else install -m755 /tmp/horcrux-guests.initd /etc/init.d/horcrux-guests; fi")
        n.reload_units()
        n.enable_unit("horcrux-guests")
        n.start_unit("horcrux-api")
        n.wait_for_unit("horcrux-api", 120)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")
        n.succeed("echo __ok__")

        env = f"HX_USER=hxadmin HX_PASS='{PW}' "

        def hx(method, path, body=None, ok=True):
            c = f"{env}python3 /root/hx_node.py api {method} {path}" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=300).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                raise AssertionError(f"{method} {path}: {r}")
            return r

        def serial(cfg, cmd):
            sock = cfg["disks"][0]["path"].rsplit(".", 1)[0] + ".serial.sock"
            return n.succeed(f"{env}python3 /root/hx_node.py serial {sock} \"{cmd}\"", timeout=900).strip().splitlines()[-1]

        def create(vm):
            hx("DELETE", f"/vms/{vm}", ok=False)
            hx("POST", "/vms", {"id": vm, "name": vm, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 1, "memory": 256, "disk_size": 1,
                                "status": "stopped", "display": {"protocol": "none"}, "hardware": {}})
            hx("POST", f"/vms/{vm}/import-disk", {"source": "/root/cirros.img"})
            hx("POST", f"/vms/{vm}/start")

        def wait_status(vm, want, secs=300):
            n.wait_until(lambda: hx("GET", f"/vms/{vm}")["body"]["status"] == want, secs, 3, f"{vm} {want}")

        def gview(vm):
            return next(g for g in hx("GET", "/nodes/guest-power")["body"]["guests"] if g["id"] == vm)

        m.phase("defaults on this host")
        ov = hx("GET", "/nodes/guest-power")["body"]
        m.check("default-stop-action-is-save", ov["node"]["stop_action"] == "save", ov["node"]["stop_action"])
        m.check("default-requires-encrypted-storage", ov["node"]["save_encryption"] == "require", "")
        m.check("save-target-is-encrypted-luks", ov["save_target"]["encrypted"] is True and ov["save_target"]["kind"] == "luks", json.dumps(ov["save_target"]))
        m.check("stop-budget-reported", ov["stop_budget_secs"] > 0, str(ov["stop_budget_secs"]))

        m.phase("a Linux VM with a counter process, and a VM that owns passthrough hardware")
        create("vmA")
        create("vmP")
        cfgA = hx("GET", "/vms/vmA")["body"]
        marker = f"hx-ram-{int(time.time())}"
        serial(cfgA, f"echo {marker} > /dev/shm/m; nohup sh -c 'i=0; while :; do i=\\$((i+1)); echo \\$i > /dev/shm/cnt; sleep 1; done' >/dev/null 2>&1 & echo \\$! > /dev/shm/pid")
        pid = serial(cfgA, "cat /dev/shm/pid").split()[-1]
        time.sleep(20)
        c0 = int(serial(cfgA, "cat /dev/shm/cnt").split()[-1])
        up0 = float(serial(cfgA, "cut -d' ' -f1 /proc/uptime").split()[-1])
        m.note("counter-before", f"{c0} (uptime {up0}s, pid {pid})")
        n.succeed("python3 - <<'EOF'\nimport sqlite3, glob\np = [x for x in glob.glob('/var/lib/horcrux/*.db')][0]\nc = sqlite3.connect(p)\n"
                  "c.execute(\"UPDATE vms SET hardware = json_set(coalesce(hardware,'{}'), '$.hostpci', json('[{\\\"device\\\":\\\"0000:ff:ff.0\\\"}]')) WHERE id='vmP'\")\nc.commit()\nEOF")
        n.stop_unit("horcrux-api")
        n.start_unit("horcrux-api")
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")
        gA, gP = gview("vmA"), gview("vmP")
        m.check("plan-linux-vm-will-be-saved", gA["host_stop_action"] == "save", str(gA["host_stop_action"]))
        m.check("plan-passthrough-vm-will-shut-down", gP["host_stop_action"] == "shutdown" and "passthrough" in (gP["host_stop_reason"] or ""), str(gP["host_stop_reason"]))

        m.phase("saved memory is refused on unencrypted storage")
        node = hx("GET", "/nodes/guest-power")["body"]["node"]
        for where in ("/boot/hxplain", "/dev/shm/hxplain"):
            n.succeed(f"mkdir -p {where}")
            hx("PUT", "/nodes/guest-power", {**node, "save_dir": where})
            g = gview("vmA")
            m.check(f"refused-{where}", g["can_save"] is False and "encrypted" in (g["save_blocker"] or ""), str(g["save_blocker"])[:200])
            r = hx("POST", "/vms/vmA/save-state", ok=False)
            m.check(f"manual-save-409-{where}", r["status"] == 409 and "encrypted" in json.dumps(r["body"]), str(r["status"]))
        hx("PUT", "/nodes/guest-power", {**node, "save_dir": None})
        m.check("vm-kept-running-after-refusals", hx("GET", "/vms/vmA")["body"]["status"] == "running", "")

        m.phase("HOST REBOOT (the init system's own reboot): vmA saved, vmP shut down")
        n.execute("sync", timeout=30)
        try:
            n.execute("reboot", timeout=15)
        except Exception:
            pass
        n.shell_ready = n.ssh_ok = False
        time.sleep(5)
        unlock_boot()
        n.wait_for_unit("horcrux-api", 300)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 300, 3, "api health after reboot")
        wait_status("vmA", "running", 600)
        cfgA = hx("GET", "/vms/vmA")["body"]
        mark = serial(cfgA, "cat /dev/shm/m").split()[-1]
        m.check("ram-marker-survived-host-reboot", mark == marker, mark)
        m.check("same-process-still-running", serial(cfgA, f"test -d /proc/{pid} && echo alive").split()[-1] == "alive", pid)
        c1 = int(serial(cfgA, "cat /dev/shm/cnt").split()[-1])
        up1 = float(serial(cfgA, "cut -d' ' -f1 /proc/uptime").split()[-1])
        m.check("counter-continued-not-restarted", c1 >= c0 and up1 >= up0 and up1 < up0 + 900, f"counter {c0} -> {c1}, uptime {up0} -> {up1}")
        mf = json.loads(n.succeed("cat /var/lib/horcrux/saved-state/running.json"))
        outc = {o["id"]: o for o in mf["outcomes"]}
        m.check("manifest-vmA-saved", outc.get("vmA", {}).get("action") == "saved", json.dumps(outc.get("vmA")))
        m.check("manifest-vmP-shutdown-with-reason", outc.get("vmP", {}).get("action") == "shutdown" and "passthrough" in (outc["vmP"].get("message") or ""), json.dumps(outc.get("vmP")))
        m.check("saved-memory-deleted-after-resume", n.succeed("ls /var/lib/horcrux/saved-state/data 2>/dev/null | wc -l").strip() == "0", "")
        wait_status("vmP", "running", 600)   # it was running: started again after its clean shutdown

        m.phase("a save that exceeds its timeout falls back to a clean shutdown")
        node = hx("GET", "/nodes/guest-power")["body"]["node"]
        hx("PUT", "/nodes/guest-power", {**node, "save_timeout_secs": 5, "shutdown_timeout_secs": 120})
        n.succeed("cat > /root/stop-stall.sh <<'EOF'\n#!/bin/sh\n(/usr/bin/horcrux-api guests stop > /root/stop.out 2>&1) &\nfor i in $(seq 1 200); do\n  p=$(pgrep -f 'cat > .*vmstate.tmp' | head -1)\n  [ -n \"$p\" ] && { kill -STOP $p; break; }\n  sleep 0.1\ndone\nwait\npkill -CONT -f 'cat > .*vmstate.tmp' || true\nEOF\nchmod +x /root/stop-stall.sh")
        n.succeed("/root/stop-stall.sh", timeout=600)
        out = n.succeed("cat /root/stop.out")
        m.check("timeout-falls-back-to-clean-shutdown", re.search(r"shutdown vm vmA.*did not finish within 5 s", out) is not None, out[-400:])
        m.check("no-half-written-state-kept", n.succeed("ls /var/lib/horcrux/saved-state/data 2>/dev/null | wc -l").strip() == "0", "")
        hx("PUT", "/nodes/guest-power", node)
        n.screenshot("done")
        shutil.rmtree(os.path.dirname(key), ignore_errors=True)

    return Test("host-shutdown-save", nodes=[node], run=run, timeout=7200,
                description="host reboot: running VM saved to encrypted disk and resumed with memory intact; passthrough/unencrypted/timeout fallbacks")
