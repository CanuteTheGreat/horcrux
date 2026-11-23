"""Host restart with a passthrough VM: the GUEST hibernates itself and resumes where it was (Horcrux, OpenRC and systemd).

A real Horcrux ISO is installed, the manager under test is dropped in, and a Debian guest (the stock cloud image plus
qemu-guest-agent and the Horcrux guest agent, built by horcrux-api/tests/hx_hibernate_image.sh) runs inside it. The guest
is NOT prepared for hibernation: the scenario runs `horcrux-guest-agent --hibernate-enable` in it (swap file, resume= on
the kernel command line, initramfs resume support), restarts it once, and then proves with the guest's own RAM marker and
counter process that

  1. a VM that owns passthrough hardware (a fake `hostpci` entry added to a running VM) is planned as `hibernate`, with the
     reason, and the guest reports it is ready;
  2. the host stop pass (`horcrux-api guests stop`: what the init system's stop path runs) makes the guest hibernate by
     itself: the VM ends up stopped and the outcome is `hibernated`;
  3. the boot pass starts the VM again and the guest RESUMES: same marker, same process, counter continues, uptime
     continues (no fresh boot);
  4. the same across a real host restart (the VM's start action is "always", the fake device is removed while it is
     down: a fake PCI device cannot be attached to QEMU, so a VM that is still passthrough could not start again);
  5. with guest hibernate switched off for the VM, the same VM is shut down cleanly instead and the reason says so.

    python3 -m vmtest run scenarios/host_shutdown_hibernate.py --arg iso=/iso/horcrux-server-openrc-daily-x86_64.iso \\
        --arg bin=/w/horcrux-api --arg image=/w/debian-hib.qcow2 --arg repo=/repo --out DIR

`bin` is a horcrux-api built for the guest (any recent Linux glibc), `image` the Debian guest, `repo` the horcrux checkout.
"""
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

HTTP_PORT = 18098
VM = "vmH"


def define(args):
    iso, binary, image, repo = args["iso"], args["bin"], args["image"], args["repo"]
    answers, key = make_answers("horcrux")
    node = NodeSpec(name="hx", mem=int(args.get("mem", 6144)), cpus=4, iso=iso, iso_media="usb", firmware="uefi",
                    disks=[Disk("32G")], extra_drives=[(answers, "usb")],
                    nics=[Nic("user", forwards={"ssh": 22, "web": 8006})], ssh_key=key, console="none", boot_timeout=600,
                    cpu="host")          # the inner VM gets KVM when the runner nests, TCG otherwise

    def run(m):
        n = m.hx
        m.phase("install")
        unattended_install(m, n)
        m.phase("first boot")
        n.start()
        n.ssh_ok = False
        n.wait_for_ssh(900)
        m.check("host-boots-and-ssh", True, "")
        n.wait_for_unit("horcrux-api", 300)
        m.check("init-system", n.init.name in ("openrc", "systemd"), n.init.name)

        # the first-boot job restarts services (sshd among them) while it runs: let it finish before touching the host
        n.wait_until(lambda: n.execute("pgrep -f '[h]orcrux-firstboot'", timeout=30)[0] != 0, 900, 5, "first-boot job finished")
        n.wait_for_ssh(300)
        m.phase("drop in the manager under test and the guest image")
        d = tempfile.mkdtemp(prefix="hxsrv-")
        for src, name in ((binary, "horcrux-api"), (image, "guest.qcow2"),
                          (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py"),
                          (os.path.join(repo, "horcrux-api/tests/hx_qga.py"), "hx_qga.py"),
                          (os.path.join(repo, "deploy/openrc/horcrux-guests"), "horcrux-guests.initd"),
                          (os.path.join(repo, "deploy/systemd/horcrux-guests.service"), "horcrux-guests.service"),
                          (os.path.join(repo, "gentoo/app-emulation/horcrux-bin/files/horcrux-api.service"), "horcrux-api.service")):
            try:
                os.link(src, os.path.join(d, name))
            except OSError:
                shutil.copy(src, os.path.join(d, name))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(1)
            base = f"http://10.0.2.2:{HTTP_PORT}"
            n.succeed("mkdir -p /var/lib/horcrux/images")
            for f, dst, mode in (("horcrux-api", "/tmp/horcrux-api.new", "755"), ("guest.qcow2", "/var/lib/horcrux/images/guest.qcow2", "644"),
                                 ("hx_node.py", "/root/hx_node.py", "755"), ("hx_qga.py", "/root/hx_qga.py", "755"),
                                 ("horcrux-guests.initd", "/tmp/horcrux-guests.initd", "755"), ("horcrux-guests.service", "/tmp/horcrux-guests.service", "644"),
                                 ("horcrux-api.service", "/tmp/horcrux-api.service", "644")):
                n.succeed(f"curl -sf --retry 3 {base}/{f} -o {dst} && chmod {mode} {dst}", timeout=900)
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)
        n.stop_unit("horcrux-api")
        try:
            n.execute("pkill -x horcrux-api; true", timeout=30)      # a leftover from the first-boot job; a dropped ssh here is harmless
        except Exception:
            pass
        n.wait_for_ssh(120)
        n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
        n.succeed("if [ -d /run/systemd/system ]; then install -m644 /tmp/horcrux-guests.service /etc/systemd/system/horcrux-guests.service; "
                  "install -m644 /tmp/horcrux-api.service /etc/systemd/system/horcrux-api.service; "
                  "else install -m755 /tmp/horcrux-guests.initd /etc/init.d/horcrux-guests; fi")
        n.reload_units()
        n.enable_unit("horcrux-guests")
        n.start_unit("horcrux-api")
        n.wait_for_unit("horcrux-api", 120)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")
        n.start_unit("horcrux-guests")

        env = f"HX_USER=hxadmin HX_PASS='{PW}' "

        def hx(method, path, body=None, ok=True):
            c = f"{env}python3 /root/hx_node.py api {method} {path}" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=300).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                raise AssertionError(f"{method} {path}: {r}")
            return r

        def sock():
            cfg = hx("GET", f"/vms/{VM}")["body"]
            return cfg["disks"][0]["path"].rsplit(".", 1)[0] + ".qga.sock"

        def gexec(cmd, timeout=120):
            """Run a shell command in the guest through the guest agent; (exit code, stdout)."""
            out = n.succeed(f"python3 /root/hx_qga.py {sock()} exec --timeout {timeout} /bin/sh -c {shlex.quote(cmd)}", timeout=timeout + 60).splitlines()
            code = int(out[-1].split("=", 1)[1])
            return code, "\n".join(out[:-1]).strip()

        def agent(flag, timeout=300):
            code, out = gexec(f"/usr/bin/horcrux-guest-agent {flag}", timeout)
            line = [x for x in out.splitlines() if x.startswith("{")][-1] if out else "{}"
            return code, json.loads(line)

        def wait_guest(secs=600):
            n.wait_until(lambda: n.execute(f"python3 /root/hx_qga.py {sock()} ping", timeout=30)[0] == 0, secs, 5, "guest agent answers")

        def wait_status(want, secs=300):
            n.wait_until(lambda: hx("GET", f"/vms/{VM}")["body"]["status"] == want, secs, 3, f"{VM} {want}")

        def gview():
            return next(g for g in hx("GET", "/nodes/guest-power")["body"]["guests"] if g["id"] == VM)

        def set_hostpci(on):
            expr = ("json_set(coalesce(hardware,'{}'), '$.hostpci', json('[{\\\"device\\\":\\\"0000:ff:ff.0\\\"}]'))" if on
                    else "json_remove(coalesce(hardware,'{}'), '$.hostpci')")
            n.succeed("python3 - <<'EOF'\nimport sqlite3, glob\np = [x for x in glob.glob('/var/lib/horcrux/*.db') if sqlite3.connect(x).execute(\"select count(*) from sqlite_master where name='vms'\").fetchone()[0]][0]\n"
                      f"c = sqlite3.connect(p)\nc.execute(\"UPDATE vms SET hardware = {expr} WHERE id='{VM}'\")\nc.commit()\nEOF")
            n.restart_unit("horcrux-api")
            n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")

        m.phase("a Debian guest with the Horcrux guest agent")
        hx("POST", "/vms", {"id": VM, "name": VM, "hypervisor": "qemu", "architecture": "x86_64", "cpus": 2, "memory": 768, "disk_size": 1,
                            "status": "stopped", "display": {"protocol": "none"}, "hardware": {"guest_agent": True}})
        hx("POST", f"/vms/{VM}/import-disk", {"source": "/var/lib/horcrux/images/guest.qcow2"})
        hx("POST", f"/vms/{VM}/start")
        wait_status("running")
        wait_guest()
        _, st = agent("--hibernate-status")
        m.check("fresh-guest-reports-why-it-cannot-hibernate", st.get("ready") is False and "swap" in (st.get("reason") or ""), json.dumps(st))

        m.phase("--hibernate-enable prepares the guest (swap file, resume=, initramfs); one restart finishes it")
        code, en = agent("--hibernate-enable", 600)
        m.check("hibernate-enable-ok", code == 0 and en.get("ok") is True, json.dumps(en)[:600])
        _, st = agent("--hibernate-status")
        m.check("restart-needed-after-enable", st.get("ready") is False and "restart" in (st.get("reason") or ""), json.dumps(st))
        try:
            gexec("sync; systemctl poweroff", 60)
        except Exception:      # noqa: BLE001  the guest may go down before it reports the exit of its own poweroff
            pass
        wait_status("stopped", 300)
        hx("POST", f"/vms/{VM}/start")
        wait_status("running")
        wait_guest()
        _, st = agent("--hibernate-status")
        m.check("guest-ready-after-restart", st.get("ready") is True, json.dumps(st))

        m.phase("guest memory the test can recognise after a resume")
        marker = f"hx-hib-{int(time.time())}"
        gexec(f"echo {marker} > /dev/shm/m; nohup sh -c 'i=0; while :; do i=$((i+1)); echo $i > /dev/shm/cnt; sleep 1; done' >/dev/null 2>&1 & echo $! > /dev/shm/pid")

        def probe():
            # read-only, so it may be asked again: the guest agent can stall for a while right after a resume
            o, last = "", None
            for _ in range(8):
                try:
                    _, o = gexec("cat /dev/shm/m /dev/shm/pid /dev/shm/cnt; cut -d' ' -f1 /proc/uptime; test -d /proc/$(cat /dev/shm/pid) && echo alive", 60)
                    break
                except Exception as e:      # noqa: BLE001
                    last = e
                    time.sleep(10)
            else:
                raise AssertionError(f"guest did not answer after a resume: {last}")
            f = o.split()
            return {"marker": f[0], "pid": f[1], "cnt": int(f[2]), "up": float(f[3]), "alive": f[-1] == "alive"}
        time.sleep(15)
        before = probe()
        m.note("guest-before", True, json.dumps(before))
        m.check("counter-is-running", before["alive"] and before["cnt"] > 5, json.dumps(before))

        m.phase("plan: a VM that owns passthrough hardware is planned as hibernate")
        g0 = gview()
        # (this host's storage is not encrypted, so a plain VM is shut down rather than saved: either way not hibernated)
        m.check("without-hostpci-the-plan-is-not-hibernate", g0["host_stop_action"] in ("save", "shutdown") and g0.get("can_hibernate") is None,
                json.dumps({k: g0.get(k) for k in ("host_stop_action", "can_hibernate")}))
        set_hostpci(True)
        g = gview()
        m.check("plan-is-hibernate", g["host_stop_action"] == "hibernate" and g["can_hibernate"] is True, json.dumps({k: g.get(k) for k in ("host_stop_action", "can_hibernate", "hibernate_detail")}))
        m.check("plan-names-passthrough-and-method", "passthrough" in (g["host_stop_reason"] or "") and "hibernate" in (g["host_stop_reason"] or ""), str(g["host_stop_reason"]))
        ps = hx("GET", f"/vms/{VM}/hibernate-status")["body"]
        m.check("hibernate-status-route", ps["ready"] is True, json.dumps(ps))

        def resumed(tag, wait_up):
            """The guest is running again and is the SAME guest: marker, process and counter continue."""
            wait_status("running", 600)
            wait_guest(600)
            after = probe()
            m.check(f"{tag}: ram-marker-survived", after["marker"] == marker, json.dumps(after))
            m.check(f"{tag}: same-process-still-running", after["pid"] == before["pid"] and after["alive"], json.dumps(after))
            m.check(f"{tag}: counter-continued-not-restarted", after["cnt"] >= before["cnt"] and after["up"] >= before["up"] and after["up"] < before["up"] + wait_up,
                    f"counter {before['cnt']} -> {after['cnt']}, uptime {before['up']} -> {after['up']}")
            return after

        m.phase("host stop pass: the guest hibernates itself; boot pass: it resumes")
        t0 = time.time()
        out = n.succeed("/usr/bin/horcrux-api guests stop 2>&1", timeout=900)
        m.note("stop-pass-seconds", True, f"{time.time() - t0:.0f}")
        m.check("stop-output-says-hibernated", "hibernat" in out.lower(), out[-500:])
        st = hx("GET", f"/vms/{VM}")["body"]["status"]
        m.check("vm-is-stopped-by-its-guest", st == "stopped", st)
        outc = {o["id"]: o for o in (hx("GET", "/nodes/guest-power")["body"].get("last_stop") or {}).get("outcomes", [])}
        m.check("outcome-hibernated", outc.get(VM, {}).get("action") == "hibernated" and outc[VM].get("ok"), json.dumps(outc.get(VM)))
        set_hostpci(False)        # the fake device could not be attached to QEMU; the guest itself is what is under test
        r = hx("POST", "/nodes/guest-power/boot-guests")["body"]
        o = next((x for x in r["outcomes"] if x["id"] == VM), {})
        m.check("boot-pass-started-it-and-says-it-resumes", o.get("action") == "started" and "hibernation" in (o.get("message") or ""), json.dumps(o))
        resumed("stop-pass", 3600)

        m.phase("the same across a real host restart (start action: always)")
        hx("PUT", f"/vms/{VM}/power-policy", {"start_action": "always"})
        set_hostpci(True)
        n.succeed("/usr/bin/horcrux-api guests stop 2>&1", timeout=900)
        wait_status("stopped", 60)
        set_hostpci(False)
        n.succeed("sync")
        try:
            n.execute("reboot", timeout=15)          # the init system's own reboot; ssh answers again once the host is back
        except Exception:
            pass
        n.shell_ready = n.ssh_ok = False
        time.sleep(10)
        n.wait_for_ssh(900)
        n.wait_for_unit("horcrux-api", 300)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 300, 3, "api health after restart")
        resumed("host-restart", 7200)

        m.phase("guest hibernate off for the VM: the same VM is shut down cleanly, and says why")
        hx("PUT", f"/vms/{VM}/power-policy", {"start_action": "always", "guest_hibernate": False})
        set_hostpci(True)
        g = gview()
        m.check("plan-is-shutdown-when-off", g["host_stop_action"] == "shutdown" and "passthrough" in (g["host_stop_reason"] or ""), json.dumps({k: g.get(k) for k in ("host_stop_action", "host_stop_reason")}))
        n.succeed("/usr/bin/horcrux-api guests stop 2>&1", timeout=900)
        outc = {o["id"]: o for o in (hx("GET", "/nodes/guest-power")["body"].get("last_stop") or {}).get("outcomes", [])}
        m.check("outcome-shutdown-with-reason", outc.get(VM, {}).get("action") == "shutdown" and "hibernate is off" in (outc[VM].get("message") or ""), json.dumps(outc.get(VM)))
        n.screenshot("done")
        shutil.rmtree(os.path.dirname(key), ignore_errors=True)

    return Test("host-shutdown-hibernate", nodes=[node], run=run, timeout=10800,
                description="passthrough VM: guest hibernates itself at host shutdown, resumes at boot with memory intact; fallback when off")
