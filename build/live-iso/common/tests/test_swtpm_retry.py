"""Daily 19748 failed with a bare 'swtpm did not create its control socket'. ensure_swtpm() must retry a failed start and,
when it keeps failing, report the exit code and swtpm's stderr."""
import importlib.util
import os
import stat
import tempfile
import types
import unittest

COMMON = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load():
    sp = importlib.util.spec_from_file_location("hx_qt_swtpm", os.path.join(COMMON, "qemu-test.py"))
    m = importlib.util.module_from_spec(sp)
    sp.loader.exec_module(m)
    return m


class SwtpmRetry(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(__import__("shutil").rmtree, self.tmp, True)
        self.oldpath = os.environ["PATH"]
        self.addCleanup(os.environ.__setitem__, "PATH", self.oldpath)
        os.environ["PATH"] = self.tmp + os.pathsep + self.oldpath
        self.mod = load()
        import time as _t
        self.mod.time = types.SimpleNamespace(sleep=lambda s: _t.sleep(min(s, 0.01)), time=_t.time, monotonic=_t.monotonic)

    def stub(self, body):
        p = os.path.join(self.tmp, "swtpm")
        open(p, "w").write("#!/bin/sh\n" + body)
        os.chmod(p, os.stat(p).st_mode | stat.S_IXUSR)

    def obj(self):
        d = os.path.join(self.tmp, "tpmdir")
        os.makedirs(d, exist_ok=True)
        o = types.SimpleNamespace(tpm_dirs={"tpm": d}, tpm_proc=None, tpm_proc2=None)
        return o, d

    def test_second_attempt_succeeds(self):
        flag = os.path.join(self.tmp, "ran")
        self.stub(f'if [ ! -e {flag} ]; then touch {flag}; echo boom >&2; exit 1; fi\n'
                  'for a; do case "$a" in type=unixio,path=*) s=${a#type=unixio,path=};; esac; done\n'
                  ': > "$s"; sleep 5\n')
        o, d = self.obj()
        sock = self.mod.Runner.ensure_swtpm(o, "tpm")
        self.assertTrue(os.path.exists(sock))
        if o.tpm_proc:
            o.tpm_proc.kill()

    def test_failure_reports_cause(self):
        self.stub("echo 'cannot open /dev/foo' >&2; exit 3\n")
        o, d = self.obj()
        cls = self.mod.Runner
        with self.assertRaises(SystemExit) as c:
            cls.ensure_swtpm(o, "tpm")
        self.assertIn("rc=3", str(c.exception))
        self.assertIn("cannot open /dev/foo", str(c.exception))


if __name__ == "__main__":
    unittest.main()
