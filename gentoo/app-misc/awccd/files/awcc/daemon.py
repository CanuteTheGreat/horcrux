"""awccd: root daemon, D-Bus name org.horcrux.Awcc1 on the system bus (spec section 1/2).

Read-only by default for anything BIOS-side (MUX/TCC/OC are not implemented at all). Writes: platform_profile, fan boost,
lighting. State needed to undo a change is kept in /run/awccd/state.json and restored on SIGTERM / `ExecStopPost`."""
import json
import os
import signal
import sys

from . import config as cfgmod
from . import elc as elcmod
from .sysfs import Platform

BUS = "org.horcrux.Awcc1"
PATH = "/org/horcrux/Awcc1"
STATE = "/run/awccd/state.json"
XML = """<node><interface name="org.horcrux.Awcc1">
  <method name="GetStatus"><arg type="s" direction="out"/></method>
  <method name="SetProfile"><arg type="s" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <method name="SetFanBoost"><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <method name="LightingStatic"><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <method name="LightingRestore"><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
  <signal name="Changed"><arg type="s"/></signal>
</interface></node>"""


class Core:
    """all logic, no D-Bus: unit tested with a fake /sys and a fake ELC"""

    def __init__(self, root="/", cfg=None, state_path=STATE, elc=None):
        self.plat = Platform(root)
        self.cfg = cfg or cfgmod.load()
        self.state_path = state_path
        self.elc = elc
        self.state = {}
        self._load_state()

    # -- state for restore
    def _load_state(self):
        try:
            with open(self.state_path) as f:
                self.state = json.load(f)
        except (OSError, ValueError):
            self.state = {}

    def _save_state(self):
        try:
            os.makedirs(os.path.dirname(self.state_path), exist_ok=True)
            with open(self.state_path, "w") as f:
                json.dump(self.state, f)
        except OSError:
            pass

    # -- API
    def status(self):
        return {"profile": self.plat.profile(), "choices": self.plat.choices(), "fans": self.plat.fans(),
                "temps": self.plat.temps(), "backlight": self.plat.backlights(),
                "lighting": {"available": bool(self.elc and self.elc.path), "method": getattr(self.elc, "method", None)},
                "bios_writes": bool(self.cfg["safety"]["enable_bios_writes"])}

    def set_profile(self, name):
        prev = self.plat.profile()
        ok, msg = self.plat.set_profile(name)
        if ok and "profile_before" not in self.state and prev and prev != name:
            self.state["profile_before"] = prev
            self._save_state()
        return ok, msg

    def set_fan_boost(self, fan, pct):
        ok, msg = self.plat.set_fan_boost(fan, min(int(pct), int(self.cfg["fans"]["max_boost"])))
        if ok:
            self.state["boost_touched"] = True
            self._save_state()
        return ok, msg

    def lighting_static(self, color, zones):
        if not self.elc:
            return False, "no lighting controller"
        try:
            rgb = elcmod.parse_hex_color(color)
            leds = elcmod.zones_to_leds(list(zones) or ["all"])
            if self.elc.fd is None:
                self.elc.open()
                self.elc.probe()
            bad = self.elc.static_color(leds, rgb)
        except (ValueError, OSError) as e:
            return False, str(e)
        if bad:
            return False, "controller rejected: " + ",".join(bad)
        self.state["lighting_changed"] = True
        self._save_state()
        return True, "ok"

    def lighting_restore(self):
        if not self.elc:
            return False, "no lighting controller"
        try:
            if self.elc.fd is None:
                self.elc.open()
                self.elc.probe()
            bad = self.elc.restore()
        except OSError as e:
            return False, str(e)
        self.state.pop("lighting_changed", None)
        self._save_state()
        return (not bad), ("ok" if not bad else "controller rejected: " + ",".join(bad))

    def apply_startup(self):
        """boot / resume: apply the configured default profile once (never overrides a profile the user set at runtime)"""
        mode = self.cfg["thermal"]["default_mode"]
        if mode and mode in self.plat.choices() and self.plat.profile() != mode and "profile_before" not in self.state:
            self.plat.set_profile(mode)

    def restore_on_exit(self):
        """spec 0.4: undo what this process changed"""
        self.plat.reset_fan_boosts()
        pb = self.state.pop("profile_before", None)
        if pb:
            self.plat.set_profile(pb)
        self._save_state()
        if self.state.pop("lighting_changed", None) and self.elc:
            try:
                self.lighting_restore()
            except Exception:       # noqa
                pass


def main(argv=None):                 # pragma: no cover (needs a bus)
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib
    cfg = cfgmod.load(os.environ.get("AWCCD_CONFIG", "/etc/awccd.toml"))
    core = Core(cfg=cfg, elc=elcmod.ELC())
    core.apply_startup()
    loop = GLib.MainLoop()
    node = Gio.DBusNodeInfo.new_for_xml(XML)

    def call(conn, sender, path, iface, method, params, inv):
        a = params.unpack()
        try:
            if method == "GetStatus":
                inv.return_value(GLib.Variant("(s)", (json.dumps(core.status()),)))
            elif method == "SetProfile":
                inv.return_value(GLib.Variant("(bs)", core.set_profile(a[0])))
            elif method == "SetFanBoost":
                inv.return_value(GLib.Variant("(bs)", core.set_fan_boost(a[0], a[1])))
            elif method == "LightingStatic":
                inv.return_value(GLib.Variant("(bs)", core.lighting_static(a[0], a[1])))
            elif method == "LightingRestore":
                inv.return_value(GLib.Variant("(bs)", core.lighting_restore()))
            else:
                inv.return_dbus_error("org.horcrux.Awcc1.Error", "unknown method")
        except Exception as e:      # noqa
            inv.return_dbus_error("org.horcrux.Awcc1.Error", str(e))

    def on_bus(conn, name):
        conn.register_object(PATH, node.interfaces[0], call, None, None)

    def on_sleep(conn, sender, path, iface, signal_name, params):
        if not params.unpack()[0]:          # PrepareForSleep(false) = resumed
            core.apply_startup()
            conn.emit_signal(None, PATH, BUS, "Changed", GLib.Variant("(s)", ("resume",)))

    last = {"p": core.plat.profile()}

    def poll():                           # firmware hotkeys can change the profile behind our back
        cur = core.plat.profile()
        if cur != last["p"]:
            last["p"] = cur
            bus.emit_signal(None, PATH, BUS, "Changed", GLib.Variant("(s)", ("profile",)))
        return True

    bus = Gio.bus_get_sync(Gio.BusType.SYSTEM, None)
    Gio.bus_own_name_on_connection(bus, BUS, Gio.BusNameOwnerFlags.NONE, on_bus, None)
    bus.signal_subscribe("org.freedesktop.login1", "org.freedesktop.login1.Manager", "PrepareForSleep",
                         "/org/freedesktop/login1", None, Gio.DBusSignalFlags.NONE, on_sleep)
    GLib.timeout_add_seconds(5, poll)

    def stop(*_):
        core.restore_on_exit()
        loop.quit()
    for s in (signal.SIGTERM, signal.SIGINT):
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, s, stop)
    loop.run()
    return 0


if __name__ == "__main__":           # pragma: no cover
    sys.exit(main())
