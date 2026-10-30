"""awccctl: command line client of awccd (falls back to read-only sysfs when the daemon is not running)."""
import argparse
import json
import sys

from .sysfs import Platform


def _proxy():
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio
    return Gio.DBusProxy.new_for_bus_sync(Gio.BusType.SYSTEM, Gio.DBusProxyFlags.NONE, None,
                                          "org.horcrux.Awcc1", "/org/horcrux/Awcc1", "org.horcrux.Awcc1", None)


def _call(method, fmt=None, *args):
    from gi.repository import GLib
    p = _proxy()
    r = p.call_sync(method, GLib.Variant(fmt, args) if fmt else None, 0, 30000, None)
    return r.unpack()


def status():
    try:
        return json.loads(_call("GetStatus")[0]), "awccd"
    except Exception:       # noqa  daemon down: read-only direct view
        p = Platform()
        return {"profile": p.profile(), "choices": p.choices(), "fans": p.fans(), "temps": p.temps(),
                "backlight": p.backlights()}, "sysfs (awccd not running)"


def main(argv=None):
    ap = argparse.ArgumentParser(prog="awccctl")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("status")
    sp = sub.add_parser("profile"); sp.add_argument("name", nargs="?")
    sf = sub.add_parser("fan-boost"); sf.add_argument("fan", type=int); sf.add_argument("percent", type=int)
    sl = sub.add_parser("light"); sl.add_argument("color", help="RRGGBB, or 'restore'"); sl.add_argument("zones", nargs="*", default=["shelf", "alienhead"])
    a = ap.parse_args(argv)
    if a.cmd == "status":
        st, src = status()
        print(f"source: {src}\nprofile: {st['profile']}  (choices: {' '.join(st['choices'])})")
        for f in st["fans"]:
            print(f"fan{f['id']} {f['label']}: {f['rpm']} rpm (max {f['max']}, boost {f['boost']})")
        for t in st["temps"]:
            print(f"temp{t['id']} {t['label']}: {t['celsius']} C")
        print("backlight:", ", ".join(f"{k} {v['brightness']}/{v['max']}" for k, v in st["backlight"].items()) or "none")
        return 0
    if a.cmd == "profile" and not a.name:
        st, _ = status()
        print(st["profile"])
        return 0
    if a.cmd == "profile":
        ok, msg = _call("SetProfile", "(s)", a.name)
    elif a.cmd == "fan-boost":
        ok, msg = _call("SetFanBoost", "(uu)", a.fan, a.percent)
    elif a.color == "restore":
        ok, msg = _call("LightingRestore")
    else:
        ok, msg = _call("LightingStatic", "(sas)", a.color, a.zones)
    print(msg)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
