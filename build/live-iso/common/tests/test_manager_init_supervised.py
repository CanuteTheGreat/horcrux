"""The manager is the HA supervisor of its node: a killed/crashed horcrux-api must come back by itself, and a node that joined a cluster must start it at boot
(lab finding 2026-11-06, HA suite on the three test laptops)."""
import os
import re
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))


class ManagerSupervised(unittest.TestCase):
    def test_init_script_is_supervised(self):
        s = open(os.path.join(ROOT, "build/live-iso/project/overlay/etc/init.d/horcrux-api")).read()
        self.assertRegex(s, r"(?m)^supervisor=supervise-daemon$")
        self.assertRegex(s, r"(?m)^respawn_max=0$")
        self.assertNotIn("command_background", s, "command_background is for unsupervised daemons")
        self.assertIn("output_log=", s)

    def test_cluster_create_and_join_enable_boot_start(self):
        s = open(os.path.join(ROOT, "horcrux-api/src/api/cluster_join.rs")).read()
        self.assertIn('"rc-update"', s)
        self.assertEqual(len(re.findall(r"enable_boot_start\(\)\.await;", s)), 2, "create AND join must enable the manager at boot")


if __name__ == "__main__":
    unittest.main()
