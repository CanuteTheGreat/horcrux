"""HOGSMEADE runs CI runner guests only while the owner is away: app-misc/hogsmeade-ci-idle-gate (hogsmeade variant local repo, never public).
Behaviour tests drive the real gate script with fake loginctl/gdbus/nvidia-smi/ps/curl/pgrep/runuser and a fake /proc; packaging tests pin the caps
(<= half the cores, >= 16 GB free, no GPU passthrough, drain < 5 min), init neutrality and the no-leak rules. Reads the Horcrux repo tree; skipped in Patronus's vendored common/."""
import os
import re
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-ci-idle-gate"
FILES = PKG / "files"
GATE = FILES / "hogsmeade-ci-gate"
NOW = 1_800_000_000


def _fake(d, name, body):
    p = Path(d) / "bin" / name
    p.parent.mkdir(exist_ok=True)
    p.write_text("#!/bin/bash\n" + body + "\n")
    p.chmod(p.stat().st_mode | stat.S_IEXEC)


@unittest.skipUnless((REPO / "gentoo").is_dir() and GATE.is_file(), "Horcrux repo tree only")
class Behaviour(unittest.TestCase):
    def machine(self, mutter_ms=3_600_000, hint="yes", since_ago=3600, game="", gpu="3, 401", comfy="", cpu_psi="0.00", avail_kb=40 * 1048576, arc=20 * 2**30,
                cores=24, own="", gnome=True):
        d = tempfile.mkdtemp()
        self.addCleanup(subprocess.run, ["rm", "-rf", d])
        proc = Path(d) / "proc"
        (proc / "pressure").mkdir(parents=True); (proc / "spl/kstat/zfs").mkdir(parents=True); (proc / "4242").mkdir()
        (proc / "meminfo").write_text(f"MemTotal: 65268892 kB\nMemAvailable: {avail_kb} kB\n")
        (proc / "cpuinfo").write_text("".join(f"processor\t: {i}\n" for i in range(cores)))
        (proc / "pressure/cpu").write_text(f"some avg10={cpu_psi} avg60=0.00 avg300=0.00 total=1\n")
        (proc / "spl/kstat/zfs/arcstats").write_text(f"name type data\nsize 4 {arc}\n")
        (proc / "4242/environ").write_bytes(b"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus\0")
        _fake(d, "pgrep", "echo 4242" if gnome else "exit 1")
        _fake(d, "runuser", 'while [ "$1" != env ]; do shift; done; shift; shift; exec "$@"')
        _fake(d, "gdbus", f"echo '(uint64 {mutter_ms},)'")
        since_us = (NOW - since_ago) * 1_000_000
        _fake(d, "loginctl", f'''case "$1" in list-sessions) echo "1832 1000 canutethegreat - -"; echo "1841 1000 canutethegreat seat0 tty9";;
show-session) if [ "$2" = 1841 ]; then printf 'Class=user\\nType=wayland\\nIdleHint={hint}\\nIdleSinceHint={since_us}\\n'; else printf 'Class=background\\nType=unspecified\\nIdleHint=no\\nIdleSinceHint=0\\n'; fi;; esac''')
        _fake(d, "ps", f"echo '/usr/bin/gnome-shell'; {'echo ' + repr(game) if game else 'true'}; {'echo ' + repr(own) if own else 'true'}")
        _fake(d, "nvidia-smi", f"echo '{gpu}'")
        _fake(d, "curl", f"echo '{comfy or chr(123) + chr(34) + 'queue_running' + chr(34) + ': [], ' + chr(34) + 'queue_pending' + chr(34) + ': []}'}'")
        (Path(d) / "run").mkdir()
        return d

    def gate(self, d, *args):
        env = dict(os.environ, PATH=f"{d}/bin:" + os.environ["PATH"], HCI_PROC=f"{d}/proc", HCI_NOW=str(NOW), HCI_CONF="/nonexistent",
                   HCI_RUN=f"{d}/run", HCI_ARC_PARAM=f"{d}/arc_max")
        Path(f"{d}/arc_max").write_text("21474836480\n")
        r = subprocess.run(["bash", str(GATE), *args], env=env, capture_output=True, text=True, timeout=30)
        return r.returncode, (r.stdout + r.stderr).strip()

    def test_idle_owner_nothing_running_allows_start(self):
        self.assertEqual(self.gate(self.machine(), "check", "start"), (0, "ok"))

    def test_recent_input_blocks_start_even_if_logind_says_idle(self):
        rc, out = self.gate(self.machine(mutter_ms=14 * 60 * 1000), "check", "start")
        self.assertEqual(rc, 1); self.assertIn("owner active", out)

    def test_exactly_15_minutes_idle_is_enough(self):
        self.assertEqual(self.gate(self.machine(mutter_ms=15 * 60 * 1000), "check", "start")[0], 0)

    def test_logind_not_idle_blocks_start_video_inhibitor(self):
        rc, out = self.gate(self.machine(hint="no"), "check", "start")
        self.assertEqual(rc, 1); self.assertIn("logind", out)

    def test_without_gnome_shell_logind_age_decides(self):
        self.assertEqual(self.gate(self.machine(gnome=False, since_ago=20 * 60), "check", "start")[0], 0)
        rc, out = self.gate(self.machine(gnome=False, since_ago=5 * 60), "check", "start")
        self.assertEqual(rc, 1); self.assertIn("logind", out)

    def test_game_blocks(self):
        for g in ("/home/x/.steam/steamapps/common/Game/game.exe", "gamescope -f -- foo"):
            rc, out = self.gate(self.machine(game=g), "check", "start")
            self.assertEqual(rc, 1, g); self.assertIn("game", out)

    def test_gpu_and_comfyui_block(self):
        self.assertIn("GPU", self.gate(self.machine(gpu="90, 12000"), "check", "start")[1])
        self.assertIn("GPU", self.gate(self.machine(gpu="3, 9000"), "check", "start")[1])
        self.assertIn("ComfyUI", self.gate(self.machine(comfy='{"queue_running": [[1, "x"]], "queue_pending": []}'), "check", "start")[1])

    def test_cpu_pressure_and_his_own_jobs_block_start_only(self):
        d = self.machine(cpu_psi="40.00", own="restic restore latest")
        self.assertEqual(self.gate(d, "check", "start")[0], 1)
        self.assertEqual(self.gate(d, "check", "run")[0], 0)      # a running guest is not drained by its own load

    def test_memory_keeps_16_gb_free(self):
        rc, out = self.gate(self.machine(avail_kb=20 * 1048576, arc=8 * 2**30), "check", "start")   # 20 GB - 16 = 4 GB < guest minimum
        self.assertEqual(rc, 1); self.assertIn("memory", out)
        _, caps = self.gate(self.machine(avail_kb=40 * 1048576, arc=8 * 2**30), "caps")
        mem = int(re.search(r"mem_mb=(\d+)", caps).group(1))
        self.assertLessEqual(mem + 1536, (40 - 16) * 1024)         # guest + overhead leaves >= 16 GB

    def test_arc_counts_as_reclaimable_memory(self):
        _, a = self.gate(self.machine(avail_kb=14 * 1048576, arc=20 * 2**30), "caps")
        _, b = self.gate(self.machine(avail_kb=14 * 1048576, arc=8 * 2**30), "caps")
        self.assertGreater(int(a.split("mem_mb=")[1]), int(b.split("mem_mb=")[1]))

    def test_at_most_half_the_cores(self):
        self.assertIn("cpus=12 ", self.gate(self.machine(cores=24), "caps")[1] + " ")
        self.assertIn("cpus=6 ", self.gate(self.machine(cores=12), "caps")[1] + " ")

    def test_run_mode_drains_when_owner_returns(self):
        self.assertEqual(self.gate(self.machine(mutter_ms=3000), "check", "run")[0], 1)
        self.assertEqual(self.gate(self.machine(mutter_ms=5 * 60 * 1000), "check", "run")[0], 0)   # idle but not yet 15 min: a running guest keeps going
        self.assertEqual(self.gate(self.machine(game="gamescope x"), "check", "run")[0], 1)
        self.assertEqual(self.gate(self.machine(avail_kb=10 * 1048576), "check", "run")[0], 1)

    def test_arc_shrink_and_restore_roundtrip(self):
        d = self.machine()
        self.gate(d, "arc", "shrink")
        self.assertEqual(Path(f"{d}/arc_max").read_text().strip(), str(8 * 2**30))
        self.gate(d, "arc", "restore")
        self.assertEqual(Path(f"{d}/arc_max").read_text().strip(), "21474836480")


@unittest.skipUnless((REPO / "gentoo").is_dir() and GATE.is_file(), "Horcrux repo tree only")
class Packaging(unittest.TestCase):
    def setUp(self):
        self.eb = (PKG / "hogsmeade-ci-idle-gate-1.ebuild").read_text()
        self.gate = GATE.read_text()
        self.guard = (FILES / "hogsmeade-ci-guard").read_text()
        self.conf = (FILES / "hogsmeade-ci-guard.confd").read_text()

    def test_caps_idle_and_drain_defaults(self):
        for k, v in (("IDLE_MIN", "15"), ("MIN_FREE_GB", "16"), ("CORES_FRAC_DEN", "2")):
            self.assertRegex(self.conf, re.compile(rf"^{k}={v}$", re.M), k)
        self.assertLess(int(re.search(r"^DRAIN_AFTER_S=(\d+)", self.conf, re.M).group(1)), 300)
        self.assertIn("IdleHint", self.gate); self.assertIn("IdleSinceHint", self.gate); self.assertIn("GetIdletime", self.gate)

    def test_conf_is_valid_systemd_environmentfile(self):
        for l in self.conf.splitlines():
            self.assertTrue(not l.strip() or l.startswith("#") or re.fullmatch(r"[A-Z_]+=\S*", l), l)   # systemd keeps a trailing '# comment' as part of the value

    def test_no_gpu_no_gui_no_session_touching(self):
        code = self.gate + self.guard + "".join(p.read_text() for p in FILES.iterdir() if p.is_file())
        code = "\n".join(l for l in code.splitlines() if not l.lstrip().startswith("#"))
        self.assertNotRegex(code, r"--gpus|vfio|passthrough|nvidia-container")
        self.assertNotRegex(code, r"notify-send|zenity|gtk-launch|xdg-open|dbus-launch|gsettings|systemctl --user|rc-service --user|loginctl (terminate|kill|lock)")
        self.assertRegex(self.gate, r"--method org\.gnome\.Mutter\.IdleMonitor\.GetIdletime")   # the only gdbus call: read-only

    def test_guard_only_stops_guest_containers_after_the_grace(self):
        self.assertIn('GUEST_RE:=cirunner-', self.guard)
        self.assertRegex(self.guard, r'docker ps -q --filter "name=\$\{GUEST_RE\}"')
        self.assertNotRegex(self.guard, r"docker (kill|stop|rm)[^\n]*(\$\(docker ps -aq\)|-a\b)")
        self.assertIn("DRAIN_AFTER_S", self.guard)
        self.assertIn("arc restore", self.guard)

    def test_init_neutral(self):
        self.assertTrue((FILES / "hogsmeade-ci-guard.initd").is_file())
        self.assertTrue((FILES / "hogsmeade-ci-guard.service").is_file())
        self.assertIn("EnvironmentFile=-/etc/conf.d/hogsmeade-ci-guard", (FILES / "hogsmeade-ci-guard.service").read_text())
        self.assertIn("supervisor=supervise-daemon", (FILES / "hogsmeade-ci-guard.initd").read_text())
        self.assertNotRegex(self.gate + self.guard, r"\bsystemctl\b|\brc-service\b|journalctl")

    def test_image_layer_installs_and_enables_it(self):
        for d in ("Dockerfile", "Dockerfile.part2"):
            self.assertIn("app-misc/hogsmeade-ci-idle-gate", (HSM / d).read_text(), d)
        self.assertIn("hogsmeade-ci-guard", (HSM / "Dockerfile.part2").read_text().split("rc-update add")[1] + (HSM / "Dockerfile.part2").read_text())

    def test_leak_absent_from_public_images(self):
        self.assertIn("app-misc/hogsmeade-ci-idle-gate", (LIVE / "project" / "public-forbidden.txt").read_text().split())
        self.assertIn("app-misc/hogsmeade-ci-idle-gate*", (REPO / "gentoo" / "app-misc" / "horcrux-test-machine" / "files" / "personal-apps.denylist").read_text().split())
        pat = re.compile(r"hogsmeade-ci-(idle-gate|gate|guard)")
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == Path(__file__).name:
                    continue
                if HSM in p.parents or p.name in ("public-forbidden.txt", "personal-apps.denylist", "personal-paths.denylist"):
                    continue
                try:
                    if pat.search(p.read_text()):
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [])


if __name__ == "__main__":
    unittest.main()
