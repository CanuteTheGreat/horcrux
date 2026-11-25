#!/usr/bin/env python3
"""Unit tests of games-util/horcrux-game-scale (no Steam, no mutter, no gamescope needed).

    python3 -m unittest discover -s gentoo/games-util/horcrux-game-scale/tests
"""
import importlib.machinery
import importlib.util
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest

FILES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "files")


def load(name):
    path = os.path.join(FILES, name)
    loader = importlib.machinery.SourceFileLoader(name.replace("-", "_"), path)
    spec = importlib.util.spec_from_loader(loader.name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


gs = load("horcrux-game-scale")
slo = load("horcrux-steam-launch-options")

MON = {"connector": "eDP-1", "width": 2560, "height": 1600, "refresh": 165.0, "scale": 1.3333333, "primary": True, "transform": 0}
ENV = {"WAYLAND_DISPLAY": "wayland-0", "XDG_SESSION_TYPE": "wayland", "SteamAppId": "550"}
CHAIN = ["/s/SteamLinuxRuntime_sniper/_v2-entry-point", "--verb=waitforexitandrun", "--",
         "/s/Proton/proton", "waitforexitandrun", "/s/common/Game/game.exe"]
which = lambda n: "/usr/bin/" + n  # noqa: E731


class Decide(unittest.TestCase):
    def cfg(self, **kw):
        c = dict(gs.DEFAULTS)
        c.update(kw)
        return c

    def test_scaled_monitor_wraps_whole_chain_at_panel_size(self):
        wrap, argv, reason, info = gs.decide(CHAIN, env=ENV, cfg=self.cfg(), monitors=[MON], which=which)
        self.assertTrue(wrap, reason)
        i = argv.index("--")
        self.assertEqual(argv[i + 1:], CHAIN)  # gamescope is OUTSIDE runtime + Proton
        opts = argv[:i]
        self.assertEqual(opts[0], "/usr/bin/gamescope")
        for flag, val in (("-W", "2560"), ("-H", "1600"), ("-w", "2560"), ("-h", "1600"), ("-r", "165"), ("--backend", "wayland")):
            self.assertEqual(opts[opts.index(flag) + 1], val, flag)
        self.assertIn("-f", opts)

    def test_every_tested_scale_wraps_and_scale_one_does_not(self):
        for s in (1.25, 1.3333333, 1.5, 1.75, 2.0):
            self.assertTrue(gs.decide(CHAIN, env=ENV, cfg=self.cfg(), monitors=[dict(MON, scale=s)], which=which)[0], s)
        wrap, argv, reason, _ = gs.decide(CHAIN, env=ENV, cfg=self.cfg(), monitors=[dict(MON, scale=1.0)], which=which)
        self.assertFalse(wrap)
        self.assertEqual(argv, CHAIN)
        self.assertIn("not scaled", reason)
        self.assertTrue(gs.decide(CHAIN, env=ENV, cfg=self.cfg(mode="gamescope"), monitors=[dict(MON, scale=1.0)], which=which)[0])

    def test_portrait_monitor_swaps_size(self):
        # mutter_monitors() swaps w/h for 90/270; decide() takes the logical orientation as given
        wrap, argv, _, _ = gs.decide(CHAIN, env=ENV, cfg=self.cfg(), monitors=[dict(MON, width=1600, height=2560)], which=which)
        self.assertEqual(argv[argv.index("-W") + 1], "1600")

    def test_direct_cases(self):
        cases = [
            (dict(ENV, HORCRUX_GAME_SCALE="0"), {}, "HORCRUX_GAME_SCALE"),
            (ENV, {"mode": "off"}, "mode = off"),
            (ENV, {"exclude_appids": [550]}, "exclude_appids"),
            (dict(ENV, GAMESCOPE_WAYLAND_DISPLAY="gamescope-0"), {}, "inside gamescope"),
            ({"XDG_SESSION_TYPE": "x11", "DISPLAY": ":0"}, {}, "not a Wayland session"),
        ]
        for env, kw, why in cases:
            wrap, argv, reason, _ = gs.decide(CHAIN, env=env, cfg=self.cfg(**kw), monitors=[MON], which=which)
            self.assertFalse(wrap, why)
            self.assertEqual(argv, CHAIN)
            self.assertIn(why, reason)
        wrap, _, reason, _ = gs.decide(CHAIN, env=ENV, cfg=self.cfg(), monitors=[MON], which=lambda n: None)
        self.assertFalse(wrap)
        self.assertIn("not installed", reason)
        own = ["/usr/bin/gamescope", "-f", "--"] + CHAIN
        self.assertFalse(gs.decide(own, env=ENV, cfg=self.cfg(), monitors=[MON], which=which)[0])

    def test_no_mutter_runs_directly(self):
        def boom():
            raise RuntimeError("no org.gnome.Mutter.DisplayConfig")
        orig = gs.mutter_monitors
        gs.mutter_monitors = boom
        try:
            wrap, argv, reason, _ = gs.decide(CHAIN, env=ENV, cfg=self.cfg(), which=which)
        finally:
            gs.mutter_monitors = orig
        self.assertFalse(wrap)
        self.assertIn("no monitor state from mutter", reason)

    def test_render_filter_device_and_monitor_choice(self):
        c = self.cfg(render="0.5", filter="fsr", vk_device="10de:2c58", grab_cursor=True, monitor="DP-2", extra_args=["--mangoapp"])
        other = dict(MON, connector="DP-2", primary=False, width=3840, height=2160, refresh=60.0, scale=1.5)
        wrap, argv, _, _ = gs.decide(CHAIN, env=ENV, cfg=c, monitors=[MON, other], which=which)
        opts = argv[:argv.index("--")]
        self.assertEqual(opts[opts.index("-W") + 1], "3840")
        self.assertEqual(opts[opts.index("-w") + 1], "1920")
        self.assertEqual(opts[opts.index("-h") + 1], "1080")
        self.assertEqual(opts[opts.index("-F") + 1], "fsr")
        self.assertEqual(opts[opts.index("--prefer-vk-device") + 1], "10de:2c58")
        self.assertIn("--force-grab-cursor", opts)
        self.assertEqual(opts[-1], "--mangoapp")
        self.assertEqual(gs.render_size(self.cfg(render="1920x1200"), MON), (1920, 1200))

    def test_config_file_keys_and_errors(self):
        with tempfile.TemporaryDirectory() as d:
            sysc = os.path.join(d, "sys.toml")
            with open(sysc, "w") as f:
                f.write('[wrapper]\nmode = "off"\nrender = 7\nbogus = 1\n[launch_options]\nmanage = false\n')
            old = gs.SYSTEM_CONF
            gs.SYSTEM_CONF = sysc
            try:
                cfg, errs = gs.load_config(user_conf=os.path.join(d, "none.toml"))
            finally:
                gs.SYSTEM_CONF = old
        self.assertEqual(cfg["mode"], "off")
        self.assertEqual(cfg["render"], "native")
        self.assertTrue(any("bogus" in e for e in errs) and any("render" in e for e in errs))

    def test_shipped_config_matches_defaults(self):
        import tomllib
        data = tomllib.load(open(os.path.join(FILES, "game-scale.toml"), "rb"))
        self.assertEqual(data["wrapper"], gs.DEFAULTS)
        self.assertEqual(data["launch_options"], slo.DEFAULTS)

    def test_explain_cli(self):
        env = dict(os.environ, HORCRUX_GAME_SCALE="0", HORCRUX_GAME_SCALE_SYSCONF="/nonexistent")
        out = subprocess.run([sys.executable, os.path.join(FILES, "horcrux-game-scale"), "--explain", "--", "true"], env=env,
                             capture_output=True, text=True, check=True).stdout
        j = json.loads(out)
        self.assertFalse(j["wrap"])
        self.assertEqual(j["argv"], ["true"])

    def test_direct_run_execs_game_and_keeps_exit_code(self):
        env = dict(os.environ, HORCRUX_GAME_SCALE="0", HORCRUX_GAME_SCALE_SYSCONF="/nonexistent", XDG_STATE_HOME=tempfile.mkdtemp())
        r = subprocess.run([sys.executable, os.path.join(FILES, "horcrux-game-scale"), "sh", "-c", "exit 7"], env=env)
        self.assertEqual(r.returncode, 7)

    def test_gamescope_failure_falls_back_to_the_game(self):
        with tempfile.TemporaryDirectory() as d:
            fake = os.path.join(d, "gamescope")
            with open(fake, "w") as f:
                f.write("#!/bin/sh\necho gamescope-failed >&2\nexit 1\n")
            os.chmod(fake, 0o755)
            conf = os.path.join(d, "c.toml")
            with open(conf, "w") as f:
                f.write(f'[wrapper]\nmode = "gamescope"\ngamescope = "{fake}"\n')
            mark = os.path.join(d, "ran")
            # mutter is not reachable here, so force a monitor through a tiny sitecustomize shim
            shim = os.path.join(d, "shim")
            os.makedirs(shim)
            with open(os.path.join(shim, "sitecustomize.py"), "w") as f:
                f.write("")
            code = ("import importlib.machinery,importlib.util,sys\n"
                    f"l=importlib.machinery.SourceFileLoader('g',{os.path.join(FILES, 'horcrux-game-scale')!r})\n"
                    "s=importlib.util.spec_from_loader('g',l);m=importlib.util.module_from_spec(s);l.exec_module(m)\n"
                    f"m.mutter_monitors=lambda:[{MON!r}]\n"
                    "sys.exit(m.main(['horcrux-game-scale']+sys.argv[1:]))\n")
            env = dict(os.environ, HORCRUX_GAME_SCALE_SYSCONF=conf, WAYLAND_DISPLAY="wayland-0", XDG_SESSION_TYPE="wayland",
                       XDG_STATE_HOME=d)
            env.pop("GAMESCOPE_WAYLAND_DISPLAY", None)
            r = subprocess.run([sys.executable, "-c", code, "sh", "-c", f"touch {mark}; exit 3"], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 3, r.stderr)
            self.assertTrue(os.path.exists(mark))
            log = slo.read(os.path.join(d, "horcrux", "game-scale.log"))
            self.assertIn("wrap=yes", log)
            self.assertIn("running the game directly", log)


LOCALCONFIG = '''"UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"550"
					{
						"LastPlayed"		"1700000000"
						"LaunchOptions"		"-novid"
					}
					"782330"
					{
						"LaunchOptions"		"PROTON_LOG=1 %command% +com_skipIntroVideo 1"
					}
					"1493710"
					{
						"cloud"		"x"
					}
					"377160"
					{
						"LaunchOptions"		"gamescope -f -- %command%"
					}
				}
			}
		}
	}
	"friends"
	{
		"name \\"quoted\\""		"a\\\\b"
	}
}
'''


def acf(appid, name, inst):
    return f'"AppState"\n{{\n\t"appid"\t\t"{appid}"\n\t"name"\t\t"{name}"\n\t"installdir"\t\t"{inst}"\n}}\n'


class LaunchOptions(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.mkdtemp()
        self.old = dict(os.environ)
        os.environ["HOME"] = self.home
        os.environ["XDG_STATE_HOME"] = os.path.join(self.home, ".local", "state")
        os.environ["XDG_CONFIG_HOME"] = os.path.join(self.home, ".config")
        self.root = os.path.join(self.home, ".local", "share", "Steam")
        cfgd = os.path.join(self.root, "userdata", "42", "config")
        os.makedirs(cfgd)
        self.lc = os.path.join(cfgd, "localconfig.vdf")
        with open(self.lc, "w") as f:
            f.write(LOCALCONFIG)
        sa = os.path.join(self.root, "steamapps")
        for appid, name, inst, tool in (("550", "Left 4 Dead 2", "Left 4 Dead 2", False), ("782330", "DOOM Eternal", "DOOMEternal", False),
                                        ("377160", "Fallout 4", "Fallout 4", False), ("1493710", "Proton Experimental", "Proton - Experimental", True),
                                        ("9000", "New Game", "NewGame", False)):
            os.makedirs(os.path.join(sa, "common", inst))
            if tool:
                open(os.path.join(sa, "common", inst, "toolmanifest.vdf"), "w").write('"manifest" { "commandline" "/proton %verb%" }')
            open(os.path.join(sa, f"appmanifest_{appid}.acf"), "w").write(acf(appid, name, inst))
        for name in ("steam_running", "load_config"):
            self.addCleanup(setattr, slo, name, getattr(slo, name))
        slo.steam_running = lambda roots: False
        self.cfg = dict(slo.DEFAULTS)

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.old)

    def opts(self):
        apps = slo.apps_section(slo.vdf_loads(slo.read(self.lc)))
        return {k: slo.get(v, "LaunchOptions") for k, v in apps}

    def test_apply_remove_roundtrip(self):
        msgs = []
        n = slo.edit(self.cfg, "apply", out=msgs.append)
        self.assertEqual(n, 3, msgs)
        o = self.opts()
        self.assertEqual(o["550"], "horcrux-game-scale %command% -novid")
        self.assertEqual(o["782330"], "PROTON_LOG=1 horcrux-game-scale %command% +com_skipIntroVideo 1")
        self.assertEqual(o["9000"], "horcrux-game-scale %command%")       # installed, no entry yet: entry created
        self.assertIsNone(o["1493710"])                                   # a tool (toolmanifest.vdf): untouched
        self.assertEqual(o["377160"], "gamescope -f -- %command%")        # already gamescope: untouched
        # other sections survive, escapes included
        tree = slo.vdf_loads(slo.read(self.lc))
        fr = slo.child(slo.child(tree, "UserLocalConfigStore"), "friends")
        self.assertEqual(fr, [['name "quoted"', "a\\b"]])
        self.assertEqual(slo.edit(self.cfg, "apply", out=msgs.append), 0)  # idempotent
        self.assertTrue([p for p in os.listdir(os.path.dirname(self.lc)) if p.endswith(".bak")])
        self.assertEqual(slo.edit(self.cfg, "remove", out=msgs.append), 3)
        o = self.opts()
        self.assertEqual(o["550"], "-novid")
        self.assertEqual(o["782330"], "PROTON_LOG=1 %command% +com_skipIntroVideo 1")
        self.assertEqual(o["9000"], "")

    def test_not_while_steam_runs_and_excludes(self):
        slo.steam_running = lambda roots: True
        before = slo.read(self.lc)
        self.assertEqual(slo.edit(self.cfg, "apply", out=lambda m: None), -1)
        self.assertEqual(slo.read(self.lc), before)
        slo.steam_running = lambda roots: False
        self.assertEqual(slo.edit(dict(self.cfg, exclude_appids=[550, 9000]), "apply", out=lambda m: None), 1)
        self.assertEqual(self.opts()["550"], "-novid")

    def test_steam_style_expansion_runs_the_wrapper_outermost(self):
        """Steam replaces %command% with the compat chain and applies leading VAR=value words as environment."""
        slo.edit(self.cfg, "apply", out=lambda m: None)
        words = shlex.split(self.opts()["782330"])
        env = {}
        while words and "=" in words[0] and not words[0].startswith("%"):
            k, v = words.pop(0).split("=", 1)
            env[k] = v
        i = words.index("%command%")
        argv = words[:i] + CHAIN + words[i + 1:]
        self.assertEqual(env, {"PROTON_LOG": "1"})
        self.assertEqual(argv[0], "horcrux-game-scale")
        self.assertEqual(argv[1:1 + len(CHAIN)], CHAIN)

    def test_watch_once_applies(self):
        slo.load_config = lambda: (dict(self.cfg), [])
        slo.watch(self.cfg, out=lambda m: None, once=True)
        self.assertTrue(self.opts()["9000"].startswith("horcrux-game-scale"))

    def test_bad_vdf_left_untouched(self):
        with open(self.lc, "w") as f:
            f.write('"UserLocalConfigStore" { "a" ')
        msgs = []
        self.assertEqual(slo.edit(self.cfg, "apply", out=msgs.append), 0)
        self.assertTrue(any("cannot parse" in m for m in msgs))
        self.assertEqual(slo.read(self.lc), '"UserLocalConfigStore" { "a" ')


if __name__ == "__main__":
    unittest.main()
