"""common/image-name.sh: the <product>-<desktop env|server>-<init>-<channel>-<arch> naming scheme (and the `desktop` alias)."""
import os, subprocess, unittest

SH = os.path.join(os.path.dirname(__file__), "..", "image-name.sh")


def names(variant="", init="", arch="x86_64"):
    env = {"PATH": os.environ["PATH"], "VARIANT": variant, "ARCH": arch}
    if init:
        env["INIT"] = init
    out = subprocess.run(["bash", "-c", f'. {SH}; echo "$VARIANT|$IMG_DE|$IMG_INIT|$IMG_HW|$IMG|$IMG_ID|$IMG_OLD_VPFX"'],
                         env=env, capture_output=True, text=True, check=True).stdout.strip()
    return out.split("|")


class ImageName(unittest.TestCase):
    def test_server_default(self):
        self.assertEqual(names(), ["", "server", "openrc", "", "server-openrc", "server-openrc-x86_64", ""])
        self.assertEqual(names("server")[4], "server-openrc")

    def test_desktop_alias_is_gnome_light(self):
        for v in ("desktop", "gnome-light"):
            self.assertEqual(names(v), ["gnome-light", "gnome-light", "openrc", "", "gnome-light-openrc", "gnome-light-openrc-x86_64", "desktop-"])

    def test_hardware_variant_is_not_a_desktop_env(self):
        self.assertEqual(names("desktop,t2")[:5], ["gnome-light,t2", "gnome-light", "openrc", "t2-", "gnome-light-openrc"])
        self.assertEqual(names("t2")[1], "server")

    def test_future_de_and_init(self):
        self.assertEqual(names("xfce", "systemd", "aarch64")[4:6], ["xfce-systemd", "xfce-systemd-aarch64"])
        self.assertEqual(names("kde")[4], "kde-openrc")

    def test_unknown_init_refused(self):
        r = subprocess.run(["bash", "-c", f". {SH}"], env={"PATH": os.environ["PATH"], "INIT": "runit"}, capture_output=True, text=True)
        self.assertNotEqual(r.returncode, 0)


if __name__ == "__main__":
    unittest.main()
