"""Horcrux web UI signs in SYSTEM users (PAM): no 'admin' account, no first-boot password, the installer creates the user.
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)"""
import os
import sys
import tempfile
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import desktop  # noqa: E402
from rescue import ops  # noqa: E402

PROFILE = os.path.join(HERE, "..", "..", "project", "overlay", "etc", "live-installer", "profile.toml")


# Horcrux-only: the vendored copy in Patronus has no build/live-iso tree and its profile.toml has no web_auth key.
_HORCRUX_TREE = os.path.isdir(os.path.join(HERE, "..", "..", "..", "..", "build", "live-iso"))


@unittest.skipUnless(_HORCRUX_TREE, "Horcrux repo tree only")
class HorcruxProfile(unittest.TestCase):
    def setUp(self):
        self.p = tomllib.load(open(PROFILE, "rb"))

    def test_uses_pam_and_has_no_first_boot_admin_password(self):
        self.assertEqual(self.p.get("web_auth"), "pam")
        self.assertNotIn("admin_password_file", self.p)
        self.assertNotIn("/var/lib/horcrux/initial-admin-password", self.p.get("factory_reset_files", []))

    def test_installer_user_lands_in_wheel_and_horcrux_admin(self):
        self.assertIn("wheel", desktop.GROUPS)
        self.assertIn("horcrux-admin", desktop.GROUPS)

    def test_admin_and_root_are_not_acceptable_user_names(self):
        self.assertTrue(desktop.user_problem("root"))
        self.assertFalse(desktop.user_problem("canutethegreat"))


class ProductAdminGroup(unittest.TestCase):
    """a product that is not Horcrux (Patronus) names its own web-admin group in the profile"""

    def test_default_and_override(self):
        self.assertEqual(desktop.admin_group({}), "horcrux-admin")
        self.assertEqual(desktop.admin_group({"web_admin_group": "patronus-admin"}), "patronus-admin")

    def test_rescue_finds_admins_of_the_given_group(self):
        with tempfile.TemporaryDirectory() as r:
            os.makedirs(r + "/etc")
            open(r + "/etc/group", "w").write("wheel:x:10:\npatronus-admin:x:990:alice\nhorcrux-admin:x:991:bob\n")
            open(r + "/etc/passwd", "w").write("alice:x:1001:1001::/h:/bin/sh\nbob:x:1002:1002::/h:/bin/sh\n")
            self.assertEqual(ops.pam_admin_users(r, "patronus-admin"), ["alice"])
            self.assertEqual(ops.pam_admin_users(r), ["bob"])


class RescueResetsSystemUsers(unittest.TestCase):
    def test_pam_admin_users_are_login_users_in_wheel_or_horcrux_admin(self):
        with tempfile.TemporaryDirectory() as r:
            os.makedirs(r + "/etc")
            open(r + "/etc/group", "w").write("root:x:0:root\nwheel:x:10:root,ron\nhorcrux-admin:x:990:alice\nusers:x:100:bob\n")
            open(r + "/etc/passwd", "w").write("root:x:0:0::/root:/bin/bash\nron:x:1000:1000::/home/ron:/bin/bash\n"
                                               "alice:x:1001:1001::/home/alice:/bin/bash\nbob:x:1002:100::/home/bob:/bin/bash\n")
            self.assertEqual(ops.pam_admin_users(r), ["alice", "ron"])


if __name__ == "__main__":
    unittest.main()
