import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from awcc import elc, sysfs, daemon, config  # noqa: E402


def w(root, rel, text):
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(text)


def fake_sys(root, profile="balanced"):
    w(root, "sys/firmware/acpi/platform_profile", profile + "\n")
    w(root, "sys/firmware/acpi/platform_profile_choices", "quiet cool balanced balanced-performance performance low-power\n")
    h = "sys/class/hwmon/hwmon7/"
    w(root, h + "name", "alienware_wmi\n")
    for i, (lab, rpm) in enumerate((("CPU Fan 1", 1800), ("CPU Fan 2", 1900), ("GPU Fan 1", 2000), ("GPU Fan 2", 2100)), 1):
        w(root, h + f"fan{i}_input", f"{rpm}\n"); w(root, h + f"fan{i}_label", lab + "\n")
        w(root, h + f"fan{i}_max", "4300\n"); w(root, h + f"fan{i}_boost", "0\n")
    w(root, h + "temp1_input", "52000\n"); w(root, h + "temp1_label", "CPU\n")
    w(root, h + "temp2_input", "47000\n"); w(root, h + "temp2_label", "GPU\n")
    w(root, "sys/class/hwmon/hwmon0/name", "coretemp\n")
    w(root, "sys/class/backlight/nvidia_wmi_ec_backlight/brightness", "161\n")
    w(root, "sys/class/backlight/nvidia_wmi_ec_backlight/max_brightness", "255\n")
    w(root, "sys/class/backlight/nvidia_wmi_ec_backlight/type", "firmware\n")


class Sysfs(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory(); self.r = self.t.name; fake_sys(self.r)
        self.p = sysfs.Platform(self.r)

    def test_read(self):
        self.assertEqual(self.p.profile(), "balanced")
        self.assertIn("balanced-performance", self.p.choices())
        self.assertEqual([f["rpm"] for f in self.p.fans()], [1800, 1900, 2000, 2100])
        self.assertEqual(self.p.fans()[0]["max"], 4300)
        self.assertEqual(self.p.temps()[0]["celsius"], 52.0)
        self.assertEqual(self.p.backlights()["nvidia_wmi_ec_backlight"]["max"], 255)

    def test_set_profile_ok_and_refuses_unknown(self):
        self.assertEqual(self.p.set_profile("quiet"), (True, "ok"))
        self.assertEqual(self.p.profile(), "quiet")
        ok, msg = self.p.set_profile("turbo")
        self.assertFalse(ok); self.assertIn("not offered", msg)
        self.assertEqual(self.p.profile(), "quiet")

    def test_fan_boost_clamped_and_gmode_refused(self):
        ok, _ = self.p.set_fan_boost(2, 250)
        self.assertTrue(ok)
        self.assertEqual(open(os.path.join(self.p.hwmon(), "fan2_boost")).read(), "100")
        self.p.set_profile("performance")
        ok, msg = self.p.set_fan_boost(2, 10)
        self.assertFalse(ok); self.assertIn("G-Mode", msg)
        self.p.reset_fan_boosts()
        self.assertEqual(open(os.path.join(self.p.hwmon(), "fan2_boost")).read(), "0")


class Frames(unittest.TestCase):
    def test_static_recipe_matches_verified_log(self):
        # alienfx-protocol.md, evidence E060 (owner saw solid blue): blue on LEDs 0-30 + 32
        leds = list(range(31)) + [32]
        seq = elc.static_color_sequence(leds, (0, 0, 255))
        self.assertEqual(len(seq), 5)
        self.assertEqual(seq[0][1:9].hex(), "03210001ffff0000")             # START_NEW 0xFFFF
        self.assertEqual(seq[1][1:7].hex(), "032301001c00")                  # loop, chunk 0, 28 LEDs: 0..27
        self.assertEqual(list(seq[1][6:34]), list(range(0, 28)))
        self.assertEqual(seq[2][1:6].hex(), "0323010104")                    # chunk 1, 4 LEDs
        self.assertEqual(list(seq[2][6:10]), [28, 29, 30, 32])
        self.assertEqual(seq[3][1:11].hex(), "03240007d000fa0000ff")          # Color, 2000 ms, tempo 250, 0,0,255
        self.assertEqual(seq[4][1:9].hex(), "0321000300ff0000")              # FINISH_AND_PLAY 0x00FF
        self.assertTrue(all(len(x) == 34 and x[0] == 0 for x in seq))

    def test_restore_sequence(self):
        r = elc.restore_sequence()
        self.assertEqual(r[0][1:7].hex(), "032100050010")
        self.assertEqual([x[1:3].hex() for x in r], ["0321"] * 3)

    def test_destructive_and_persistent_commands_refused(self):
        with self.assertRaises(ValueError):
            elc.frame(3, 0xFF)
        for sub in (2, 6, 7):
            with self.assertRaises(ValueError):
                elc.anim_cmd(sub, 1)

    def test_zones_and_colour(self):
        self.assertEqual(len(elc.zones_to_leds(["all"])), 45)
        self.assertEqual(elc.zones_to_leds(["alienhead", "power"]), [32, 31])
        self.assertEqual(elc.parse_hex_color("#00ff80"), (0, 255, 128))
        with self.assertRaises(ValueError):
            elc.zones_to_leds(["lid"])
        with self.assertRaises(ValueError):
            elc.parse_hex_color("12")

    def test_find_hidraw(self):
        with tempfile.TemporaryDirectory() as r:
            w(r, "sys/class/hidraw/hidraw3/device/uevent", "DRIVER=hid-generic\nHID_ID=0003:0000187C:00000551\n")
            w(r, "sys/class/hidraw/hidraw4/device/uevent", "HID_ID=0003:00000D62:00006ABC\n")
            self.assertEqual(elc.find_hidraw(r), "/dev/hidraw3")


class FakeELC:
    path = "/dev/null"; fd = 1; method = "output"

    def __init__(self):
        self.calls = []

    def static_color(self, leds, rgb):
        self.calls.append(("static", leds, rgb)); return []

    def restore(self, anim_id=16):
        self.calls.append(("restore",)); return []


class Core(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory(); self.r = self.t.name; fake_sys(self.r)
        self.state = os.path.join(self.r, "run/awccd/state.json")
        self.elc = FakeELC()
        self.c = daemon.Core(self.r, config.load("/nonexistent"), self.state, self.elc)

    def test_status_json_serialisable(self):
        s = self.c.status(); json.dumps(s)
        self.assertEqual(s["profile"], "balanced"); self.assertFalse(s["bios_writes"])

    def test_profile_restored_on_exit(self):
        self.assertTrue(self.c.set_profile("quiet")[0])
        self.c.set_fan_boost(1, 40)
        self.assertEqual(self.c.plat.profile(), "quiet")
        self.c.restore_on_exit()
        self.assertEqual(self.c.plat.profile(), "balanced")
        self.assertEqual(open(os.path.join(self.c.plat.hwmon(), "fan1_boost")).read(), "0")

    def test_restore_survives_daemon_crash(self):
        self.c.set_profile("cool")
        c2 = daemon.Core(self.r, config.load("/nonexistent"), self.state, self.elc)     # new process reads state.json
        c2.restore_on_exit()
        self.assertEqual(c2.plat.profile(), "balanced")

    def test_lighting_calls_and_restore_on_exit(self):
        ok, _ = self.c.lighting_static("ff0000", ["alienhead"])
        self.assertTrue(ok); self.assertEqual(self.elc.calls[0], ("static", [32], (255, 0, 0)))
        self.assertFalse(self.c.lighting_static("zz", ["all"])[0])
        self.c.restore_on_exit()
        self.assertEqual(self.elc.calls[-1], ("restore",))

    def test_startup_default_mode_not_applied_over_user_choice(self):
        self.c.cfg["thermal"]["default_mode"] = "quiet"
        self.c.apply_startup(); self.assertEqual(self.c.plat.profile(), "quiet")


if __name__ == "__main__":
    unittest.main()
