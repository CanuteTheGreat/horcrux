"""zram-swap init script: mkswap/swapon failures (udev EBUSY race on a slow first boot) are retried, never swallowed."""
import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "overlay/etc/init.d/zram-swap")


def run_start(fail_mkswap):
    d = tempfile.mkdtemp()
    cnt = os.path.join(d, "n")
    stubs = {
        "modprobe": "exit 0",
        "udevadm": "exit 0",
        "sleep": "exit 0",
        "zramctl": 'echo /dev/zram0',
        "swapon": 'echo swapon "$@" >> %s/log' % d,
        "mkswap": 'n=$(cat %s 2>/dev/null || echo 0); echo $((n+1)) > %s; [ "$n" -lt %d ] && { echo "mkswap: Device or resource busy" >&2; exit 1; }; exit 0' % (cnt, cnt, fail_mkswap),
    }
    os.makedirs(os.path.join(d, "bin"))
    for k, v in stubs.items():
        f = os.path.join(d, "bin", k)
        open(f, "w").write("#!/bin/sh\n" + v + "\n")
        os.chmod(f, 0o755)
    drv = os.path.join(d, "drv.sh")
    open(drv, "w").write(
        "ebegin() { :; }\neend() { return \"${1:-0}\"; }\n"
        "sed -e 's#/run/zram-swap.dev#%s/dev#' -e 's#/etc/conf.d/zram-swap#/nonexistent#' %s > %s/s.sh\n. %s/s.sh\nstart\n" % (d, SCRIPT, d, d))
    env = dict(os.environ, PATH=os.path.join(d, "bin") + ":" + os.environ["PATH"])
    r = subprocess.run(["sh", drv], capture_output=True, text=True, env=env)
    log = open(os.path.join(d, "log")).read() if os.path.exists(os.path.join(d, "log")) else ""
    return r, log


class ZramSwapStart(unittest.TestCase):
    def test_transient_mkswap_busy_is_retried_until_swapon(self):
        r, log = run_start(3)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("swapon --priority 100 /dev/zram0", log)

    def test_permanent_failure_is_reported(self):
        r, log = run_start(10 ** 6)
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(log, "")
        self.assertIn("no working zram swap", r.stderr)


if __name__ == "__main__":
    unittest.main()
