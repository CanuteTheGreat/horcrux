"""System Log viewer on a REAL installed Horcrux ISO: journald on the systemd image, sysklogd's /var/log/messages on the OpenRC image.

A real ISO is installed, the horcrux-api under test replaces the packaged binary, then the viewer's API is exercised against the
host's own logger (`logger`, `systemd-run`) and its own kernel: structured entries, minimum priority, identifier (exact and `*`),
unit, kernel-only, relative time span, the live stream (a WebSocket client in the guest sees a line logged after it connected),
and - after the node really rebooted - the boot list and entries of the previous boot with different boot ids (what the page turns
into a reboot separator).

    python3 -m vmtest run scenarios/system_log_viewer.py --arg iso=/iso/horcrux-server-systemd-daily-x86_64.iso \\
        --arg bin=/w/horcrux-api --arg repo=/repo --out DIR       (and the same with the openrc ISO)

`bin` is a horcrux-api built for the guest (any recent Linux glibc), `repo` the horcrux checkout (hx_node.py, hx_logws.py).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PW, make_answers, unattended_install  # noqa: E402

HTTP_PORT = 18098


def define(args):
    iso, binary, repo = args["iso"], args["bin"], args["repo"]
    answers, key = make_answers("horcrux")
    node = NodeSpec(name="hx", mem=int(args.get("mem", 4096)), cpus=2, iso=iso, iso_media="usb", firmware="uefi",
                    disks=[Disk("24G")], extra_drives=[(answers, "usb")],
                    nics=[Nic("user", forwards={"ssh": 22, "web": 8006})], ssh_key=key, console="none", boot_timeout=600)

    def run(m):
        n = m.hx
        m.phase("install")
        unattended_install(m, n)
        m.phase("first boot")
        n.start()
        n.wait_for_ssh(600)
        n.wait_for_unit("horcrux-api", 300)
        init = n.init.name
        m.check("init-system", init in ("openrc", "systemd"), init)

        m.phase("drop in the manager under test")
        d = tempfile.mkdtemp(prefix="hxlog-")
        for src, name in ((binary, "horcrux-api"), (os.path.join(repo, "horcrux-api/tests/hx_node.py"), "hx_node.py"),
                          (os.path.join(repo, "horcrux-api/tests/hx_logws.py"), "hx_logws.py")):
            try:
                os.link(src, os.path.join(d, name))
            except OSError:
                shutil.copy(src, os.path.join(d, name))
        srv = subprocess.Popen([sys.executable, "-m", "http.server", str(HTTP_PORT), "--directory", d, "--bind", "0.0.0.0"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(1)
            base = f"http://10.0.2.2:{HTTP_PORT}"
            for f, dst, mode in (("horcrux-api", "/tmp/horcrux-api.new", "755"), ("hx_node.py", "/root/hx_node.py", "755"),
                                 ("hx_logws.py", "/root/hx_logws.py", "755")):
                n.succeed(f"curl -sf --retry 3 {base}/{f} -o {dst} && chmod {mode} {dst}", timeout=600)
        finally:
            srv.terminate()
            shutil.rmtree(d, ignore_errors=True)
        n.stop_unit("horcrux-api")
        n.succeed("install -m755 /tmp/horcrux-api.new /usr/bin/horcrux-api")
        n.start_unit("horcrux-api")
        n.wait_for_unit("horcrux-api", 120)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 180, 3, "api health")

        env = f"HX_USER=hxadmin HX_PASS='{PW}' "

        def hx(method, path, body=None, ok=True):
            c = f"{env}python3 /root/hx_node.py api {method} '{path}'" + (f" '{json.dumps(body)}'" if body is not None else "")
            r = json.loads(n.succeed(c, timeout=120).strip().splitlines()[-1])
            if ok and r["status"] >= 300:
                diag = n.execute("ls -la /usr/bin/horcrux-api /tmp/horcrux-api.new; ps -eo pid,lstart,args | grep '[h]orcrux-api'; "
                                 + n.init.log("horcrux-api", 25), timeout=60)[1]
                raise AssertionError(f"{method} {path}: {r}\n{diag[-2500:]}")
            return r

        def logs(q=""):
            return hx("GET", "/system/logs" + ("?" + q if q else ""))["body"]

        m.phase("which log source this image has")
        info = hx("GET", "/system/logs/info")["body"]
        if init == "systemd":
            m.check("source-is-journald", info["source"] == "journald" and not info["limitations"], json.dumps(info))
            persistent = n.execute("test -d /var/log/journal")[0] == 0
            m.note("journal-persistent-by-default", persistent, "previous boots survive a reboot only when /var/log/journal exists")
            if not persistent:
                n.succeed("mkdir -p /var/log/journal && systemd-tmpfiles --create --prefix /var/log/journal && systemctl restart systemd-journald && journalctl --flush")  # vmtest:init-ok (journald itself is under test)
        else:
            m.check("source-is-a-syslog-file", info["source"] == "syslog-file" and info["file"], json.dumps(info))
            m.check("the-logger-is-running", n.execute("pgrep -x syslogd || pgrep -x syslog-ng || pgrep -x metalog")[0] == 0,
                    n.succeed("ps -eo comm | grep -i log || true"))
            m.check("limitations-are-stated", any("priority" in x for x in info["limitations"]), "; ".join(info["limitations"]))

        m.phase("structured entries from the host's own logger")
        tag = f"hxtest{int(time.time()) % 100000}"
        n.succeed(f"logger -t {tag} -p user.err 'error: {tag} an error line'; logger -t {tag} -p user.info '{tag} an info line'; "
                  f"logger -t {tag}other -p user.warning '{tag}other a warning'")
        time.sleep(2)
        got = logs(f"identifier={tag}")
        m.check("identifier-exact", len(got) == 2 and all(e["identifier"] == tag for e in got), json.dumps(got)[:400])
        e0 = got[0]
        m.check("entry-has-structured-fields", e0["pid"] and e0["hostname"] and e0["priority"] is not None and e0["priority_name"] and e0["timestamp"], json.dumps(e0)[:300])
        got = logs(f"identifier={tag}*")
        m.check("identifier-wildcard", len(got) == 3, json.dumps([e["identifier"] for e in got]))
        got = logs(f"identifier={tag}*&priority=3")
        m.check("minimum-priority-3-keeps-the-error-only", len(got) == 1 and "an error line" in got[0]["message"], json.dumps(got)[:300])
        if init == "systemd":
            m.check("journald-priority-is-the-logged-one", got[0]["priority"] == 3 and not got[0]["priority_inferred"], str(got[0]["priority"]))
            n.succeed(f"systemd-run --unit={tag}unit --collect /bin/sh -c 'echo {tag}-unit-line'")
            time.sleep(2)
            got = logs(f"unit={tag}unit.service")
            m.check("unit-filter-finds-the-transient-unit", any(f"{tag}-unit-line" in e["message"] for e in got), json.dumps(got)[:300])
        else:
            got = logs(f"unit={tag}")
            m.check("unit-filter-matches-the-program-name", len(got) == 2, json.dumps(got)[:300])
        got = logs(f"identifier={tag}&since=1m")
        m.check("relative-since", len(got) == 2, "")
        m.check("until-in-the-past-hides-them", logs(f"identifier={tag}&until=1h") == [], "")
        r = hx("GET", "/system/logs?since=nonsense", ok=False)
        m.check("bad-time-is-400", r["status"] == 400, str(r["status"]))
        units = hx("GET", "/system/logs/units")["body"]
        m.check("unit-list", units["kind"] in ("systemd", "openrc", "identifiers") and len(units["units"]) > 0, f"{units['kind']} {len(units['units'])}")

        m.phase("kernel only")
        k = logs("kernel=true&limit=50")
        m.check("kernel-only-returns-kernel-lines", len(k) > 0 and all(e["kernel"] for e in k), f"{len(k)} lines; first: {json.dumps(k[:1])[:250]}")

        m.phase("live stream: a line logged after the client connected")
        n.succeed(f"nohup python3 /root/hx_logws.py follow 'identifier={tag}live' 25 > /tmp/live.out 2>&1 &")
        time.sleep(6)
        n.succeed(f"logger -t {tag}live -p user.notice '{tag} live line'; logger -t {tag}elsewhere 'not this stream'")
        time.sleep(8)
        out = n.succeed("cat /tmp/live.out")
        frames = [json.loads(x) for x in out.splitlines() if x.startswith("{")]
        ents = [e for f in frames if f.get("type") == "entries" for e in f["entries"]]
        m.check("live-hello", frames and frames[0].get("type") == "hello", out[:300])
        m.check("live-line-arrived-filtered", [e["message"] for e in ents] == [f"{tag} live line"], out[-400:])

        m.phase("REBOOT, then the previous boot's lines and the boot list")
        n.succeed("sync")
        before = hx("GET", "/system/logs/boots")["body"]
        try:
            n.execute("reboot", timeout=15)
        except Exception:
            pass
        n.shell_ready = n.ssh_ok = False
        time.sleep(10)
        n.wait_for_ssh(600)
        n.wait_for_unit("horcrux-api", 300)
        n.wait_until(lambda: n.http("web", "/api/health")[0] == 200, 300, 3, "api health after reboot")
        n.wait_until_succeeds("pgrep -f '[h]orcrux-firstboot' >/dev/null && exit 1; exit 0", 300, 5)
        time.sleep(5)
        boots = hx("GET", "/system/logs/boots")["body"]
        m.check("a-second-boot-is-listed", len(boots) >= max(2, len(before) + 1) and boots[-1]["index"] == 0, json.dumps(boots)[:500])
        old = logs(f"identifier={tag}&boot=-1")
        m.check("previous-boot-still-has-its-lines", len(old) == 2, json.dumps(old)[:300])
        m.check("current-boot-does-not-have-them", logs(f"identifier={tag}&boot=0") == [], "")
        allv = logs("limit=5000")
        ids = {e["boot_id"] for e in allv if e["boot_id"]}
        m.check("entries-carry-two-boot-ids-for-the-separator", len(ids) >= 2, f"{len(ids)} boot ids in {len(allv)} entries")
        k = logs("kernel=true&boot=0&limit=20")
        m.check("kernel-only-in-the-new-boot", len(k) > 0, f"{len(k)} lines")
        n.screenshot("done")
        shutil.rmtree(os.path.dirname(key), ignore_errors=True)

    return Test("system-log-viewer", nodes=[node], run=run, timeout=5400,
                description="system log viewer on a real installed ISO: journald (systemd) or sysklogd file (OpenRC), filters, live, reboot")
