"""GNOME network notifications (gnome-extra/horcrux-gnome-defaults >= 1-r14): the extension network-notify@horcrux.errant.solutions replaces
GNOME Shell's generic "Connection failed" notification. Structural checks of the package plus the extension's decision table (node or gjs);
the real NetworkManager + gnome-shell round trip is network-notify-vm-test.py (attach mode, installed GNOME VM)."""
import glob
import os
import re
import shutil
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET
import json

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "gnome-extra", "horcrux-gnome-defaults")
EXT = os.path.join(PKG, "files", "network-notify")
UUID = "network-notify@horcrux.errant.solutions"


@unittest.skipUnless(os.path.isdir(EXT), "Horcrux repo tree only")
class Package(unittest.TestCase):
    def setUp(self):
        self.ebuild = sorted(glob.glob(os.path.join(PKG, "*.ebuild")))[-1]
        self.e = open(self.ebuild).read()

    def test_revision_ships_the_extension(self):
        self.assertTrue(self.ebuild.endswith("-r14.ebuild") or re.search(r"-r(1[5-9]|[2-9]\d)\.ebuild$", self.ebuild), self.ebuild)
        self.assertIn(f"/usr/share/gnome-shell/extensions/{UUID}", self.e)
        self.assertIn("{metadata.json,extension.js,policy.js}", self.e)
        self.assertIn("gnome2_schemas_update", self.e)

    def test_enabled_for_new_and_existing_accounts(self):
        self.assertRegex(self.e, r"enabled-extensions=\['appindicatorsupport@rgcjonas\.gmail\.com', '" + re.escape(UUID) + "'")
        self.assertIn("default-extensions", self.e)
        self.assertIn("horcrux-default-extensions.desktop", self.e)
        desk = open(os.path.join(PKG, "files", "horcrux-default-extensions.desktop")).read()
        self.assertIn("OnlyShowIn=GNOME;", desk)
        self.assertTrue(os.access(os.path.join(PKG, "files", "horcrux-default-extensions"), os.X_OK))

    def test_metadata(self):
        m = json.load(open(os.path.join(EXT, "metadata.json")))
        self.assertEqual(m["uuid"], UUID)
        self.assertIn("50", m["shell-version"])
        self.assertEqual(m["settings-schema"], "org.gnome.shell.extensions.horcrux-network-notify")

    def test_schema_keys_and_defaults(self):
        root = ET.parse(os.path.join(EXT, "schemas", "org.gnome.shell.extensions.horcrux-network-notify.gschema.xml")).getroot()
        keys = {k.get("name"): k.findtext("default") for k in root.iter("key")}
        self.assertEqual(keys, {"failure-grace-seconds": "30", "notify-when-other-link-online": "false", "repeat-interval-seconds": "900"})

    def test_extension_replaces_the_stock_handler_and_reads_every_setting(self):
        js = open(os.path.join(EXT, "extension.js")).read()
        self.assertIn("_onActivationFailed = () => {}", js)
        self.assertIn("delete this._indicator._onActivationFailed", js)      # disable() puts the stock method back
        for key in ("failure-grace-seconds", "notify-when-other-link-online", "repeat-interval-seconds"):
            self.assertIn(f"'{key}'", js)
        self.assertIn("NM.DeviceStateReason.NO_SECRETS", js)                 # as stock: a cancelled password dialog is not a failure

    def test_schema_compiles(self):
        if not shutil.which("glib-compile-schemas"):
            self.skipTest("no glib-compile-schemas")
        with tempfile.TemporaryDirectory() as d:
            shutil.copy(os.path.join(EXT, "schemas", "org.gnome.shell.extensions.horcrux-network-notify.gschema.xml"), d)
            r = subprocess.run(["glib-compile-schemas", "--strict", d], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)


@unittest.skipUnless(os.path.isdir(EXT), "Horcrux repo tree only")
class Policy(unittest.TestCase):
    def test_decision_table(self):
        with tempfile.TemporaryDirectory() as d:
            shutil.copy(os.path.join(EXT, "policy.js"), os.path.join(d, "policy.mjs"))
            if shutil.which("node"):
                cmd = ["node", os.path.join(HERE, "network_notify_policy.mjs"), os.path.join(d, "policy.mjs")]
            else:
                self.skipTest("needs node")
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
            self.assertIn("PASS", r.stdout)


class DefaultExtensionsScript(unittest.TestCase):
    """horcrux-default-extensions with a fake gsettings/gnome-extensions: adds a new default once, never re-adds after the user removed it,
    never enables one listed in disabled-extensions."""
    SCRIPT = os.path.join(PKG, "files", "horcrux-default-extensions")

    def run_script(self, d, enabled, disabled="@as []"):
        open(os.path.join(d, "enabled"), "w").write(enabled)
        open(os.path.join(d, "disabled"), "w").write(disabled)
        env = dict(os.environ, PATH=f"{d}/bin:" + os.environ["PATH"], XDG_STATE_HOME=f"{d}/state", HORCRUX_DEFAULT_EXTENSIONS=f"{d}/list")
        subprocess.run(["sh", self.SCRIPT], env=env, check=True, timeout=30)
        return open(os.path.join(d, "enabled")).read()

    @unittest.skipUnless(os.path.exists(SCRIPT), "Horcrux repo tree only")
    def test_once_per_account(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/bin")
            open(f"{d}/list", "w").write(f"# comment\n{UUID}\n")
            open(f"{d}/bin/gsettings", "w").write(f"#!/bin/sh\ncase \"$1 $3\" in\n'get enabled-extensions') cat {d}/enabled;;\n"
                                                    f"'get disabled-extensions') cat {d}/disabled;;\n'set enabled-extensions') printf %s \"$4\" > {d}/enabled;;\nesac\n")
            open(f"{d}/bin/gnome-extensions", "w").write("#!/bin/sh\nexit 1\n")       # shell not reachable: the gsettings fallback is used
            for f in ("gsettings", "gnome-extensions"):
                os.chmod(f"{d}/bin/{f}", 0o755)
            self.assertEqual(self.run_script(d, "['a@b']"), f"['a@b', '{UUID}']")
            # the user turned it off afterwards: it stays off
            self.assertEqual(self.run_script(d, "['a@b']"), "['a@b']")
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/bin")
            open(f"{d}/list", "w").write(f"{UUID}\n")
            open(f"{d}/bin/gsettings", "w").write(f"#!/bin/sh\ncase \"$1 $3\" in\n'get enabled-extensions') cat {d}/enabled;;\n"
                                                    f"'get disabled-extensions') cat {d}/disabled;;\n'set enabled-extensions') printf %s \"$4\" > {d}/enabled;;\nesac\n")
            open(f"{d}/bin/gnome-extensions", "w").write("#!/bin/sh\nexit 1\n")
            for f in ("gsettings", "gnome-extensions"):
                os.chmod(f"{d}/bin/{f}", 0o755)
            self.assertEqual(self.run_script(d, "@as []"), f"['{UUID}']")
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/bin")
            open(f"{d}/list", "w").write(f"{UUID}\n")
            open(f"{d}/bin/gsettings", "w").write(f"#!/bin/sh\ncase \"$1 $3\" in\n'get enabled-extensions') cat {d}/enabled;;\n"
                                                    f"'get disabled-extensions') cat {d}/disabled;;\n'set enabled-extensions') printf %s \"$4\" > {d}/enabled;;\nesac\n")
            open(f"{d}/bin/gnome-extensions", "w").write("#!/bin/sh\nexit 1\n")
            for f in ("gsettings", "gnome-extensions"):
                os.chmod(f"{d}/bin/{f}", 0o755)
            self.assertEqual(self.run_script(d, "['a@b']", f"['{UUID}']"), "['a@b']")


if __name__ == "__main__":
    unittest.main()
