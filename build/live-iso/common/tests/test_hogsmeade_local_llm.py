"""HOGSMEADE local LLM: app-misc/hogsmeade-local-llm (hogsmeade variant local repo, never public).
Behaviour tests drive the real guard script with a fake gate, fake nvidia-smi and a fake `ollama` (a sleeping process), checking that the server is started only when the
gate allows it, stopped (VRAM freed) when it does not, kept down by foreign GPU memory, and never exposed beyond loopback. Packaging tests pin init neutrality and the
no-leak rules. Reads the Horcrux repo tree; skipped in Patronus's vendored common/."""
import os
import stat
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
HSM = REPO / "build" / "live-iso" / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-local-llm"
GUARD = PKG / "files" / "hogsmeade-llm-guard"
BIN = HSM / "local-repo" / "sci-ml" / "ollama-bin"


def _exe(p, body):
    p.write_text("#!/bin/bash\n" + body + "\n")
    p.chmod(p.stat().st_mode | stat.S_IEXEC)


@unittest.skipUnless((REPO / "gentoo").is_dir() and GUARD.is_file(), "Horcrux repo tree only")
class Behaviour(unittest.TestCase):
    def setUp(self):
        self.d = Path(tempfile.mkdtemp())
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.d)])
        self.run_dir = self.d / "run"
        _exe(self.d / "ollama", 'echo "$OLLAMA_HOST" > "$0.host"; exec sleep 300')
        self.set_gate("ok")
        self.set_gpu(total=400, ollama_pids=[])

    def set_gate(self, verdict):
        # the fake gate records the GPU env the guard gives it: our own inference must not trip the gate's GPU limits
        body = f'echo "$1 $2 util=$GPU_UTIL_MAX mem=$GPU_MEM_MAX_MB vm=$VM_MEM_MIN_MB" >> "{self.d}/gate.calls"\n'
        if verdict == "ok":
            body += "if [ \"$2\" = run ] || [ \"$2\" = start ]; then echo ok; exit 0; fi"
        else:
            body += f'echo "busy: {verdict}"; exit 1'
        _exe(self.d / "gate", body)

    def set_gpu(self, total, ollama_pids):
        rows = "\n".join(f"{p}, {m}" for p, m in ollama_pids)
        _exe(self.d / "nvidia-smi", f'case "$*" in *compute-apps*) printf "{rows}\\n";; *) echo {total};; esac')

    def guard(self, *args):
        env = dict(os.environ, HLL_CONF="/nonexistent", GATE=str(self.d / "gate"), OLLAMA_BIN=str(self.d / "ollama"), HLL_RUN=str(self.run_dir),
                   NVSMI=str(self.d / "nvidia-smi"), HLL_NO_RUNAS="1", HLL_LOG=str(self.d / "ollama.log"), STOP_GRACE_S="3", POLL_S="1",
                   OLLAMA_MODELS=str(self.d / "models"), PATH=f"{self.d}:{os.environ['PATH']}")
        return subprocess.run([str(GUARD), *args], env=env, capture_output=True, text=True, timeout=60)

    def state(self):
        return (self.run_dir / "state").read_text().split()

    def pid(self):
        try:
            return int((self.run_dir / "ollama.pid").read_text())
        except Exception:
            return None

    def alive(self, pid):
        try:
            os.kill(pid, 0)
        except OSError:
            return False
        # a killed process whose parent never reaps it (PID 1 of a CI job container is not an init) stays a zombie: kill(pid, 0) still
        # succeeds, but it is dead (live-iso run 2631: test_stop_is_prompt / test_owner_back_stops_server_and_frees_gpu)
        try:
            with open(f"/proc/{pid}/stat") as fh:
                return fh.read().rsplit(")", 1)[1].split()[0] != "Z"
        except (OSError, IndexError):
            return False

    def tearDown(self):
        self.guard("stop")

    def test_idle_starts_loopback_only(self):
        self.guard("once")
        self.assertEqual(self.state()[0], "up")
        pid = self.pid()
        self.assertTrue(pid and self.alive(pid))
        time.sleep(0.3)
        self.assertEqual((self.d / "ollama.host").read_text().strip(), "127.0.0.1:11434")
        calls = (self.d / "gate.calls").read_text()
        self.assertIn("start util=101 mem=99999999 vm=0", calls)       # own GPU use must not trip the gate

    def test_owner_back_stops_server_and_frees_gpu(self):
        self.guard("once")
        pid = self.pid()
        self.set_gate("owner active (input 3 s ago)")
        self.guard("once")
        self.assertEqual(self.state()[0], "down")
        self.assertIn("owner active", " ".join(self.state()))
        self.assertFalse(self.alive(pid))
        self.assertIn("run ", (self.d / "gate.calls").read_text().splitlines()[1])
        # and it stays down while he is around (start mode), without launching anything
        self.guard("once")
        self.assertIsNone(self.pid())

    def test_foreign_gpu_memory_keeps_it_down_but_our_model_does_not(self):
        self.set_gpu(total=12000, ollama_pids=[])
        self.guard("once")
        self.assertEqual(self.state()[0], "down")
        self.assertIn("GPU memory", " ".join(self.state()))
        # our own model resident (pid is a process whose comm is 'ollama'): not counted as foreign
        self.set_gpu(total=400, ollama_pids=[])
        self.guard("once")
        pid = self.pid()
        self.assertTrue(pid and self.alive(pid))
        self.assertEqual(self.state()[0], "up")

    def test_stop_is_prompt(self):
        self.guard("once")
        pid = self.pid()
        t = time.time()
        self.guard("stop")
        self.assertLess(time.time() - t, 8)
        self.assertFalse(self.alive(pid))


class Packaging(unittest.TestCase):
    def test_files(self):
        if not PKG.is_dir():
            self.skipTest("Horcrux repo tree only")
        f = PKG / "files"
        initd = (f / "hogsmeade-llm-guard.initd").read_text()
        unit = (f / "hogsmeade-llm-guard.service").read_text()
        conf = (f / "hogsmeade-llm-guard.confd").read_text()
        guard = GUARD.read_text()
        self.assertIn("openrc-run", initd)
        self.assertIn("ExecStart=/usr/libexec/hogsmeade-llm-guard", unit)
        self.assertIn("KillMode=control-group", unit)
        # same gate as the CI guests, never a copy of its logic
        self.assertIn("/usr/libexec/hogsmeade-ci-gate", guard)
        self.assertIn("app-misc/hogsmeade-ci-idle-gate", (PKG / "hogsmeade-local-llm-1.ebuild").read_text())
        # loopback only: no LAN listener, no 0.0.0.0
        self.assertIn("OLLAMA_HOST=127.0.0.1:11434", conf)
        for t in (guard, conf, initd, unit):
            self.assertNotIn("0.0.0.0", t)
        # poll fast enough to free the GPU well inside a minute
        self.assertLessEqual(int([l for l in conf.splitlines() if l.startswith("POLL_S=")][0].split("=")[1]), 15)
        # the guard owns the server: no standalone ollama service can run while he is active
        self.assertFalse(list(f.glob("*ollama*")))
        # HOGSMEADE only
        forb = (REPO / "build/live-iso/project/public-forbidden.txt").read_text()
        self.assertIn("app-misc/hogsmeade-local-llm", forb)
        self.assertIn("sci-ml/ollama-bin", forb)
        self.assertTrue((BIN / "Manifest").is_file())


if __name__ == "__main__":
    unittest.main()
