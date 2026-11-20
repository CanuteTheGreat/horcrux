"""[user] password_at_first_login = true: the desktop account gets an EMPTY, EXPIRED password, so the first console or greeter
login makes the user choose one (no temporary password is stored anywhere, the disk passphrase is never reused).
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import os
import sys
import tempfile
import types
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import desktop as D  # noqa: E402
from liveinst import system as S  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs", "xfs-luks"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}
DISK_PW = "Wq4#vN8-correct-horse-Zx2"   # test-only


def answers(extra_user="", extra_global=""):
    t = ("[global]\nhostname = \"laptop.example.com\"\ninstall_type = \"desktop\"\nssh_keys = [\"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest x\"]\n" + extra_global +
         "[disk]\nfilesystem = \"zfs\"\nencryption = true\npassphrase = \"" + DISK_PW + "\"\ndisks = [\"/dev/vda\"]\n"
         "[user]\nname = \"owner\"\n" + extra_user)
    f = tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False)
    f.write(t)
    f.close()
    try:
        return S.config_from_answers(PROFILE, f.name)
    finally:
        os.unlink(f.name)


class FirstLoginPassword(unittest.TestCase):
    def test_parsed_and_counts_as_login_user(self):
        c = answers("password_at_first_login = true\n")
        self.assertTrue(c["user_pw_first_login"])
        self.assertEqual(c["user_hash"], "")
        self.assertTrue(S.has_login_user(c))
        self.assertEqual(S.first_login_problems(c), [])

    def test_default_off_and_no_password_is_still_an_error(self):
        c = answers()
        self.assertFalse(c["user_pw_first_login"])
        self.assertFalse(S.has_login_user(c))

    def test_refused_with_a_password(self):
        c = answers("password_at_first_login = true\npassword = \"Bd6!hP3-tablet-user-Rk9\"\n")
        self.assertTrue(any("excludes password" in e for e in S.first_login_problems(c)))

    def test_refused_with_ssh_password_auth(self):
        c = answers("password_at_first_login = true\n", "ssh_password_auth = true\n")
        self.assertTrue(any("ssh_password_auth" in e for e in S.first_login_problems(c)))

    def test_create_user_empties_and_expires(self):
        with tempfile.TemporaryDirectory() as T:
            os.makedirs(T + "/etc")
            open(T + "/etc/group", "w").write("wheel:x:10:\nportage:x:250:\n")
            runs = []
            inst = types.SimpleNamespace(c={"user_name": "owner", "user_pw_first_login": True}, target=T, p={},
                                         run=lambda cmd, **kw: runs.append(cmd), log=lambda m: None)
            D.create_user(inst)
            self.assertIn(["passwd", "-d", "owner"], runs)
            self.assertIn(["chage", "-d", "0", "owner"], runs)
            self.assertFalse(any(r and r[0] == "chpasswd" for r in runs))


if __name__ == "__main__":
    unittest.main()
