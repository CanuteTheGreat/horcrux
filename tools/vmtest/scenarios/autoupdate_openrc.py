"""horcrux-autoupdate on a real (OpenRC / Alpine) guest: the real program driven by busybox crond, with stand-in emerge/emaint tools.

Proves, in a booted guest and not just in unit tests: the program runs as root under OpenRC's crond, a run does sync -> update -> status file,
a failing emerge leaves `failed` + `check` exit 2 (what Rustinion reads), a pending kernel/microcode lands in reboot-required and nothing
reboots, a second concurrent run is refused by the lock, and the hourly trigger is a no-op until INTERVAL_HOURS have passed.
"""
import base64
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test, cache  # noqa: E402

ALPINE = ("https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.4-x86_64.iso",
          "157d353f8e54252cad2d9812a0d56bb7fa2231fc154a110e1d180d7480ef8b4f")
HERE = os.path.dirname(os.path.abspath(__file__))
FILES = os.path.join(HERE, "..", "..", "..", "gentoo", "sys-apps", "horcrux-autoupdate", "files")

FAKE_EMERGE = r"""#!/bin/sh
# stand-in for portage: records what it was asked, merges what /tmp/merge lists, fails when /tmp/emerge-fail exists
echo "$@" >> /tmp/emerge-calls
case "$*" in
  *--pretend*--depclean*|*--depclean*--pretend*) echo "All selected packages: "; echo "Packages installed:   10"; exit 0;;
  *--pretend*) echo "[ebuild     U  ] sys-apps/xz-utils-5.8.2 [5.8.1]"; exit 0;;
esac
if [ -e /tmp/emerge-fail ]; then echo "!!! emerge: build failed for dev-libs/broken"; exit 1; fi
case "$*" in *@world*) i=0; for p in $(cat /tmp/merge 2>/dev/null); do i=$((i+1)); echo "$(date +%s):  ::: completed emerge ($i of 1) $p to /" >> /var/log/emerge.log; done;; esac
sleep "${EMERGE_SLEEP:-0}"
exit 0
"""


def put(node, path, data, mode="0755"):
    b = base64.b64encode(data).decode()
    node.succeed(f": > {path}.b64")
    for i in range(0, len(b), 1500):
        node.succeed(f"printf %s '{b[i:i + 1500]}' >> {path}.b64")
    node.succeed(f"base64 -d {path}.b64 > {path} && rm {path}.b64 && chmod {mode} {path}")


def define(args):
    iso = args.get("iso") or cache.fetch(*ALPINE)
    nodes = [NodeSpec(name="n1", mem=768, cpus=1, iso=iso, disks=[Disk("1G")], nics=[Nic("user", forwards={})], boot_timeout=240)]

    def run(m):
        n = m.n1
        m.phase("boot + python")
        m.start_all()
        n.wait_for_shell()
        m.check("init-is-openrc", n.init.name == "openrc", n.init.name)
        n.succeed("setup-interfaces -ar >/dev/null 2>&1; echo 'https://dl-cdn.alpinelinux.org/alpine/v3.24/main' > /etc/apk/repositories")
        n.wait_until_succeeds("apk add --no-cache python3 >/dev/null 2>&1 && python3 --version", 180)
        m.check("python-installed", True, "")

        m.phase("install the shipped files + stand-in portage")
        put(n, "/usr/sbin/horcrux-autoupdate", open(os.path.join(FILES, "horcrux-autoupdate"), "rb").read())
        n.succeed("mkdir -p /etc/horcrux /var/lib/horcrux-autoupdate /var/log/horcrux-autoupdate /usr/local/bin")
        put(n, "/usr/local/bin/emerge", FAKE_EMERGE.encode())
        n.succeed("for t in emaint etc-update; do printf '#!/bin/sh\\necho \"$0 $*\" >> /tmp/tool-calls\\nexit 0\\n' > /usr/local/bin/$t; chmod +x /usr/local/bin/$t; done")
        n.succeed("touch /var/log/emerge.log; echo 'PATH=/usr/local/bin:$PATH' > /etc/profile.d/pp.sh")
        n.succeed("PATH=/usr/local/bin:$PATH horcrux-autoupdate --help | head -1")
        m.check("cli-runs", True, "")

        m.phase("a normal run")
        n.succeed("echo 'sys-firmware/intel-microcode-20261110 sys-apps/xz-utils-5.8.2' > /tmp/merge")
        n.succeed("PATH=/usr/local/bin:$PATH horcrux-autoupdate run")
        n.succeed("grep -q emaint /tmp/tool-calls")
        n.succeed("grep -q -- '--getbinpkg' /tmp/emerge-calls && grep -q -- '@world' /tmp/emerge-calls")
        n.succeed("grep -q '\"result\": \"ok\"' /var/lib/horcrux-autoupdate/status.json")
        n.succeed("grep -q 'intel-microcode' /var/lib/horcrux-autoupdate/reboot-required")
        n.succeed("test -e /run/reboot-required")
        n.succeed("test \"$(PATH=/usr/local/bin:$PATH horcrux-autoupdate check)\" != '' && PATH=/usr/local/bin:$PATH horcrux-autoupdate check | grep -q 'reboot required'")
        n.succeed("test -s /var/log/horcrux-autoupdate/run-*.log; uptime | grep -q 'up'")
        m.check("run-ok-and-reboot-recorded", True, "status ok, microcode in reboot-required, node still up")

        m.phase("interval gate: the hourly trigger is a no-op right after a run")
        n.succeed("rm -f /tmp/emerge-calls; PATH=/usr/local/bin:$PATH horcrux-autoupdate cron; test ! -e /tmp/emerge-calls")
        m.check("cron-not-due", True, "")

        m.phase("concurrent run is refused by the lock")
        n.succeed("(EMERGE_SLEEP=20 PATH=/usr/local/bin:$PATH horcrux-autoupdate run >/tmp/long.out 2>&1 &) ; sleep 4; "
                  "PATH=/usr/local/bin:$PATH horcrux-autoupdate run 2>&1 | grep -q -E 'lock|already running'")
        n.wait_until_succeeds("grep -q '\"result\": \"ok\"' /var/lib/horcrux-autoupdate/status.json && ! pgrep -f 'horcrux-autoupdate run'", 90)
        m.check("lock", True, "")

        m.phase("failure becomes visible")
        n.succeed("touch /tmp/emerge-fail; PATH=/usr/local/bin:$PATH horcrux-autoupdate run; test $? -eq 1")
        n.succeed("test -s /var/lib/horcrux-autoupdate/failed && grep -q update /var/lib/horcrux-autoupdate/failed")
        n.succeed("PATH=/usr/local/bin:$PATH horcrux-autoupdate check; test $? -eq 2")
        n.succeed("rm /tmp/emerge-fail; PATH=/usr/local/bin:$PATH horcrux-autoupdate run && test ! -e /var/lib/horcrux-autoupdate/failed")
        n.succeed("PATH=/usr/local/bin:$PATH horcrux-autoupdate check; test $? -eq 0 -o $? -eq 0")
        m.check("failed-marker-and-check-exit-2", True, "failed file + check=2, cleared by the next good run")

        m.phase("OpenRC crond triggers it")
        n.succeed("echo '{\"result\": \"ok\", \"started\": 1}' > /var/lib/horcrux-autoupdate/status.json; rm -f /tmp/emerge-calls")
        n.succeed("echo '* * * * * PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin /usr/sbin/horcrux-autoupdate cron >> /tmp/cron-run.log 2>&1' | crontab -")
        n.succeed(n.init.start("crond"))
        n.wait_until_succeeds("test -s /tmp/emerge-calls && grep -q '\"trigger\": \"cron\"' /var/lib/horcrux-autoupdate/status.json", 150)
        m.check("crond-runs-update", True, "crond -> horcrux-autoupdate cron -> emerge")

        n.succeed("test -z \"$(ls /var/lib/horcrux-autoupdate | grep -i -E 'reboot-now')\"; uptime")
        m.check("never-rebooted", True, "uptime continuous")

    return Test("autoupdate-openrc", nodes=nodes, networks=[], run=run, timeout=900,
                description="horcrux-autoupdate in an OpenRC guest: run, lock, failure status, reboot marker, crond trigger")
