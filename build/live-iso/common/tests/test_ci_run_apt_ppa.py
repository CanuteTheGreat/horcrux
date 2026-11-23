"""ci-run.sh host deps: Launchpad PPA lists are dropped and apt is retried (run 25118: git-core PPA 404 failed `deps`)."""
import pathlib
import unittest

SRC = (pathlib.Path(__file__).resolve().parents[1] / "ci-run.sh").read_text()


class HostDeps(unittest.TestCase):
    def test_ppa_lists_removed_before_update(self):
        self.assertIn("rm -f /etc/apt/sources.list.d/*ppa*", SRC)
        self.assertLess(SRC.index("sources.list.d/*ppa*"), SRC.index("apt-get update"))

    def test_install_is_retried(self):
        self.assertIn("for attempt in 1 2 3", SRC)
        self.assertIn('fail deps "apt install failed"', SRC)


if __name__ == "__main__":
    unittest.main()
