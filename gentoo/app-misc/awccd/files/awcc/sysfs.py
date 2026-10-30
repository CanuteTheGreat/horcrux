"""Kernel-interface backends (spec section 2, [K]): platform_profile, hwmon (alienware_wmi), backlight.

Everything takes a `root` so the unit tests run against a fake /sys tree. No WMI function id is ever called from
userspace: the kernel driver alienware-wmi owns that (allow-list principle of the spec)."""
import glob
import os

PROFILE = "sys/firmware/acpi/platform_profile"
CHOICES = "sys/firmware/acpi/platform_profile_choices"
# profiles the daemon may set (the kernel only offers what the firmware advertises)
KNOWN = ("low-power", "cool", "quiet", "balanced", "balanced-performance", "performance", "custom")
GMODE_PROFILE = "performance"       # alienware-wmi gmode quirk: "performance" also switches G-Mode on (verified 0xAB path)
MAX_BOOST = 100                      # spec: clamp 0..100 until the EC clamp is verified on hardware


def _read(path):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return None


def _write(path, text):
    with open(path, "w") as f:
        f.write(text)


class Platform:
    def __init__(self, root="/"):
        self.root = root

    def p(self, rel):
        return os.path.join(self.root, rel)

    # ---- thermal profile
    def profile(self):
        return _read(self.p(PROFILE))

    def choices(self):
        t = _read(self.p(CHOICES))
        return t.split() if t else []

    def set_profile(self, name):
        """set + read back; returns (ok, message). Never writes a name the kernel did not advertise."""
        if name not in self.choices():
            return False, f"profile {name!r} not offered (choices: {' '.join(self.choices()) or 'none'})"
        try:
            _write(self.p(PROFILE), name)
        except OSError as e:
            return False, f"write failed: {e}"
        got = self.profile()
        if got != name:
            return False, f"readback mismatch: wanted {name}, kernel says {got}"
        return True, "ok"

    # ---- hwmon
    def hwmon(self):
        """the hwmon directory of the alienware_wmi driver, or None"""
        for d in sorted(glob.glob(self.p("sys/class/hwmon/hwmon*"))):
            if (_read(os.path.join(d, "name")) or "").startswith("alienware"):
                return d
        return None

    def fans(self):
        h = self.hwmon()
        out = []
        if not h:
            return out
        for f in sorted(glob.glob(os.path.join(h, "fan*_input"))):
            n = os.path.basename(f)[3:].split("_")[0]
            rpm = _read(f)
            out.append({"id": int(n), "label": _read(os.path.join(h, f"fan{n}_label")) or f"fan{n}",
                        "rpm": int(rpm) if rpm and rpm.isdigit() else None,
                        "max": _int(_read(os.path.join(h, f"fan{n}_max"))),
                        "boost": _int(_read(os.path.join(h, f"fan{n}_boost")))})
        return out

    def temps(self):
        h = self.hwmon()
        out = []
        if not h:
            return out
        for f in sorted(glob.glob(os.path.join(h, "temp*_input"))):
            n = os.path.basename(f)[4:].split("_")[0]
            v = _int(_read(f))
            out.append({"id": int(n), "label": _read(os.path.join(h, f"temp{n}_label")) or f"temp{n}",
                        "celsius": v / 1000.0 if v is not None else None})
        return out

    def set_fan_boost(self, fan, percent):
        h = self.hwmon()
        if not h:
            return False, "no alienware_wmi hwmon device"
        if self.profile() == GMODE_PROFILE:
            return False, "refused: G-Mode (performance) already runs the fans at full speed"
        path = os.path.join(h, f"fan{int(fan)}_boost")
        if not os.path.exists(path):
            return False, f"fan {fan} has no boost control"
        pct = max(0, min(MAX_BOOST, int(percent)))
        try:
            _write(path, str(pct))
        except OSError as e:
            return False, f"write failed: {e}"
        return True, f"fan{fan} boost {pct}"

    def reset_fan_boosts(self):
        h = self.hwmon()
        if not h:
            return
        for f in glob.glob(os.path.join(h, "fan*_boost")):
            try:
                _write(f, "0")
            except OSError:
                pass

    # ---- backlight (read only: GNOME owns brightness; we only report which device exists)
    def backlights(self):
        res = {}
        for d in sorted(glob.glob(self.p("sys/class/backlight/*"))):
            res[os.path.basename(d)] = {"brightness": _int(_read(os.path.join(d, "brightness"))),
                                        "max": _int(_read(os.path.join(d, "max_brightness"))),
                                        "type": _read(os.path.join(d, "type"))}
        return res


def _int(s):
    try:
        return int(s)
    except (TypeError, ValueError):
        return None
