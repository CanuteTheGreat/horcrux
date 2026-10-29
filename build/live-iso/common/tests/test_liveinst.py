"""Unit tests for the shared guided installer (pure logic; no disks touched).
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import os
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
from liveinst import system as S  # noqa: E402
from liveinst import consolemenu as CM  # noqa: E402
from liveinst import stages as ST  # noqa: E402
from liveinst import useflags as UF  # noqa: E402

PROFILE = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs"], "swap_mb": 0, "network_mode": "management",
           "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}
FW = dict(PROFILE, name="patronus", title="Patronus", filesystems=["xfs", "ext4"], network_mode="wan-lan")
GOOD_PW = "Kx9!mQv2-tle-77Zp"
HASH = "$6$salt$" + "a" * 86


class Passwords(unittest.TestCase):
    def test_accepts_strong(self):
        self.assertIsNone(S.password_problem(GOOD_PW, "node1.example.com"))

    def test_rejects_short_and_defaults(self):
        self.assertIn("at least 12", S.password_problem("Short1!"))
        self.assertIn("guessable", S.password_problem("horcrux-horcrux1A"))
        self.assertIn("guessable", S.password_problem("Patronus-2026!!"))
        self.assertIsNotNone(S.password_problem("aaaaaaaaaaaaaaaa"))
        self.assertIsNotNone(S.password_problem("alllowercaseletters"))
        self.assertIsNotNone(S.password_problem("abcdef123456XYZ!"))   # alphabet run

    def test_hostname_in_password(self):
        self.assertIn("host name", S.password_problem("Qm7!gulliver-77Zz", "gulliver.example.com"))

    def test_hash_never_plaintext(self):
        self.assertIsNone(S.crypt_hash_problem(HASH))
        self.assertIsNotNone(S.crypt_hash_problem("hunter2hunter2"))
        self.assertIsNotNone(S.crypt_hash_problem(""))


class Network(unittest.TestCase):
    def test_cidr(self):
        self.assertIsNone(S.cidr_problem("192.168.1.10/24"))
        self.assertIsNotNone(S.cidr_problem("192.168.1.10"))
        self.assertIsNotNone(S.cidr_problem("192.168.1.0/24"))
        self.assertIsNotNone(S.cidr_problem("10.0.0.1/31"))

    def test_gateway_inside(self):
        self.assertIsNone(S.ip_problem("10.0.2.2", "10.0.2.77/24"))
        self.assertIsNotNone(S.ip_problem("10.0.3.2", "10.0.2.77/24"))

    def test_hostname(self):
        self.assertIsNone(S.hostname_problem("node1.example.com"))
        self.assertIsNotNone(S.hostname_problem("Bad_Name"))
        self.assertIsNotNone(S.hostname_problem(""))

    def test_part_path(self):
        self.assertEqual(S.part_path("/dev/vda", 4), "/dev/vda4")
        self.assertEqual(S.part_path("/dev/nvme0n1", 4), "/dev/nvme0n1p4")
        self.assertEqual(S.part_path("/dev/mmcblk0", 2), "/dev/mmcblk0p2")


class Answers(unittest.TestCase):
    def cfg(self, text, profile=PROFILE):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write(textwrap.dedent(text))
        try:
            return S.config_from_answers(profile, f.name)
        finally:
            os.unlink(f.name)

    def test_needs_secret(self):
        c = self.cfg("""
            [global]
            hostname = "a.example.com"
            [disk]
            disks = ["/dev/vda"]
        """)
        errs = S.validate_config(PROFILE, c)
        self.assertTrue(any("unreachable" in e for e in errs), errs)

    def test_plaintext_password_refused(self):
        c = self.cfg("""
            [global]
            hostname = "a.example.com"
            root_password_hash = "plainpassword"
            [disk]
            disks = ["/dev/vda"]
        """)
        self.assertTrue(any("never a plain password" in e for e in S.validate_config(PROFILE, c)))

    def test_hash_ok_and_defaults_secure(self):
        c = self.cfg(f"""
            [global]
            hostname = "a.example.com"
            root_password_hash = "{HASH}"
            [disk]
            disks = ["/dev/vda"]
        """)
        self.assertEqual(S.validate_config(PROFILE, c), [])
        self.assertFalse(c["ssh_password_auth"], "password SSH must default to OFF")

    def test_zfs_mirror_needs_two_disks(self):
        c = self.cfg(f"""
            [global]
            hostname = "a.example.com"
            root_password_hash = "{HASH}"
            [disk]
            filesystem = "zfs"
            zfs_raid = "mirror"
            disks = ["/dev/vda"]
        """)
        self.assertTrue(any("two different disks" in e for e in S.validate_config(PROFILE, c)))

    def test_patronus_wan_lan_sections(self):
        c = self.cfg(f"""
            [global]
            hostname = "fw.example.com"
            root_password_hash = "{HASH}"
            [disk]
            disks = ["/dev/vda"]
            [network.wan]
            mode = "static"
            address = "203.0.113.2/29"
            gateway = "203.0.113.1"
            dns = ["9.9.9.9"]
            [network.lan]
            address = "192.168.77.1/24"
        """, FW)
        self.assertEqual(S.validate_config(FW, c), [])
        self.assertEqual(c["lan"]["address"], "192.168.77.1/24")
        self.assertEqual(c["wan"]["dns"], "9.9.9.9")

    def test_filesystem_not_offered(self):
        c = self.cfg(f"""
            [global]
            hostname = "fw.example.com"
            root_password_hash = "{HASH}"
            [disk]
            filesystem = "zfs"
            disks = ["/dev/vda"]
        """, FW)
        self.assertTrue(any("not offered" in e for e in S.validate_config(FW, c)))


class ConsoleMenuLogic(unittest.TestCase):
    def test_dhcpcd_static_replaces_installer_stanza(self):
        txt = "hostname\n\ninterface eth0\nstatic ip_address=10.0.0.5/24\nstatic routers=10.0.0.1\n\ninterface eth1\nstatic ip_address=1.1.1.1/24\n"
        out = CM.apply_iface_config(txt, "eth0", "static", "192.168.9.2/24", "192.168.9.1", "9.9.9.9")
        self.assertNotIn("10.0.0.5", out)
        self.assertIn("static ip_address=1.1.1.1/24", out)           # other NIC untouched
        self.assertIn("static ip_address=192.168.9.2/24", out)
        again = CM.apply_iface_config(out, "eth0", "dhcp")           # idempotent switch back to DHCP
        self.assertNotIn("192.168.9.2", again)
        self.assertEqual(again.count("BEGIN console-menu eth0"), 1)

    def test_roles(self):
        t = CM.apply_roles("[general]\nx = 1\n[network]\nlan_interface = \"old\"\n", "eth0", "eth1")
        self.assertEqual(CM.read_roles(t), ("eth0", "eth1"))
        self.assertEqual(t.count("[network]"), 1)
        t2 = CM.apply_roles("", "eth9", "")
        self.assertEqual(CM.read_roles(t2), ("eth9", ""))

    def test_host_validation(self):
        for good in ("example.com", "10.0.0.1", "fe80::1", "a"):
            self.assertTrue(CM.host_ok(good), good)
        for bad in ("", "-oProxy", "a b", "x;reboot", "$(id)", "a" * 300):
            self.assertFalse(CM.host_ok(bad), bad)

    def test_backup_member_paths(self):
        roots = ["/etc/horcrux", "/var/lib/horcrux/horcrux.db"]
        self.assertTrue(CM.member_allowed("etc/horcrux/ssl/a.pem", roots))
        self.assertTrue(CM.member_allowed("var/lib/horcrux/horcrux.db", roots))
        self.assertTrue(CM.member_allowed("MANIFEST.json", roots))
        for bad in ("etc/passwd", "etc/horcrux/../shadow", "../etc/shadow", "etc/horcruxevil/x", "root/.ssh/authorized_keys"):
            self.assertFalse(CM.member_allowed(bad, roots), bad)


class ConsoleMenuFlows(unittest.TestCase):
    """Drive the real Console class with scripted input against a temp sqlite DB."""
    def setUp(self):
        import sqlite3
        self.d = tempfile.mkdtemp()
        self.db = os.path.join(self.d, "x.db")
        con = sqlite3.connect(self.db)
        con.execute("create table users (username text, password_hash text)")
        con.execute("insert into users values ('admin','OLD')")
        con.execute("create table sessions (id text)")
        con.execute("insert into sessions values ('s1')")
        con.commit()
        con.close()
        self.prof = dict(PROFILE, db_path=self.db, db_table="users", web_service="", config_paths=[self.d + "/etc", self.db],
                         admin_password_file=self.d + "/first-pw", admin_password_owner="root")
        self.orig = CM.argon2_encode
        CM.argon2_encode = lambda pw, run=None: "$argon2id$stub$" + pw

    def tearDown(self):
        CM.argon2_encode = self.orig

    def console(self, answers):
        it = iter(answers)
        import io
        c = CM.Console(self.prof, inp=lambda prompt="": next(it), out=io.StringIO())
        c.secret = lambda prompt: next(it)
        return c

    def test_reset_password_updates_db_and_drops_sessions(self):
        import sqlite3
        c = self.console([GOOD_PW, GOOD_PW, "n"])
        c.reset_password()
        con = sqlite3.connect(self.db)
        self.assertEqual(con.execute("select password_hash from users").fetchone()[0], "$argon2id$stub$" + GOOD_PW)
        self.assertEqual(con.execute("select count(*) from sessions").fetchone()[0], 0)

    def test_weak_password_rejected_three_times_changes_nothing(self):
        import sqlite3
        c = self.console(["short", "short", "short"])
        c.reset_password()
        self.assertEqual(sqlite3.connect(self.db).execute("select password_hash from users").fetchone()[0], "OLD")

    def test_backup_restore_roundtrip_and_refusals(self):
        import sqlite3
        os.makedirs(self.d + "/etc")
        open(self.d + "/etc/a.conf", "w").write("one")
        c = self.console([])
        c.backup_dir = self.d + "/bk"
        b = c.make_backup("t")
        ok, msg = c.check_backup(b)
        self.assertTrue(ok, msg)
        # an archive for another product, and one with a path escape, are refused
        import json as _j, tarfile as _t, io as _io
        for man, member, why in (({"project": "patronus"}, "etc/x", "not horcrux"), ({"project": "horcrux"}, "etc/passwd", "outside")):
            fn = self.d + "/evil.tar.gz"
            with _t.open(fn, "w:gz") as tf:
                for name, data in (("MANIFEST.json", _j.dumps(man).encode()), (member, b"x")):
                    ti = _t.TarInfo(name)
                    ti.size = len(data)
                    tf.addfile(ti, _io.BytesIO(data))
            ok, msg = c.check_backup(fn)
            self.assertFalse(ok, why)


class ProfileDefaults(unittest.TestCase):
    def test_console_open_from_answers(self):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nconsole_menu_open = true\n')
        self.assertTrue(S.config_from_answers(PROFILE, f.name)["console_open"])


PROFILE_X = dict(PROFILE, services=["horcrux-api", "docker", "sshd"], services_optional=["docker"], title="Horcrux")
FIXTURE = os.path.join(HERE, "fixtures", "meta-stage3.json.xz")


def tiny_meta():
    return UF.Meta({"desc": {"a": "flag a", "b": "flag b", "c": "flag c", "x": "flag x", "feat": "feature"}, "local": {}, "pkgs": {
        "cat/A": {"v": "1", "iuse": ["a", "b", "c"], "use": ["a"], "ru": "^^ ( a b ) c? ( a )", "size": 5_000_000,
                  "cons": [[[], "cat/B[x]"]]},
        "cat/B": {"v": "1", "iuse": ["x"], "use": ["x"], "size": 1_000_000},
        "cat/D": {"v": "1", "iuse": ["feat"], "use": [], "size": 2_000_000, "cons": [[["feat"], "!cat/E"]]},
        "cat/E": {"v": "1", "iuse": [], "use": [], "size": 100_000},
    }})


class Stages(unittest.TestCase):
    def test_stage_ids_and_orders(self):
        self.assertEqual(ST.UI_ORDER, ["keymap", "hostname", "disk", "extract", "rootpass", "desktop", "t2", "gnome", "netconfig", "services", "hardening", "use_flags", "finish"])
        self.assertEqual(sorted(ST.APPLY_ORDER), sorted(ST.UI_ORDER))
        self.assertEqual(ST.APPLY_ORDER[:2], ["disk", "extract"], "disks first, then the image, then everything that writes into it")

    def test_install_type_default_server_workstation_refused(self):
        c = S.default_config(PROFILE_X)
        self.assertEqual(c["install_type"], "server")
        c["install_type"] = "workstation"
        c["disks"] = ["/dev/vda"]
        c["root_hash"] = HASH
        self.assertTrue(any("not available yet" in e for e in S.validate_config(PROFILE_X, c)))
        f = ST.BY_ID["keymap"].fields(ST.Ctx(PROFILE_X, c))
        it = next(x for x in f if x.key == "install_type")
        self.assertIn("workstation", it.disabled)
        self.assertIn("workstation", ST.TYPE_DEFAULTS, "the stage design keeps a place for a Workstation profile")

    def test_secure_defaults(self):
        c = S.default_config(PROFILE_X)
        self.assertFalse(c["ssh_password_auth"])
        self.assertFalse(c["console_open"])
        self.assertTrue(c["sysctl_hardening"])
        self.assertEqual(c["use"], {"preset": "recommended", "flags": {}})

    def test_plan_is_pure_and_complete(self):
        c = S.default_config(PROFILE_X)
        c["disks"] = ["/dev/vda"]
        ctx = ST.Ctx(PROFILE_X, c, disks=[{"path": "/dev/vda", "size": 64 * 10**9, "parts": []}], efi=True, ram_mb=8192)
        before = repr(c)
        plan = ST.full_plan(ctx)
        self.assertEqual(repr(c), before, "plan() must not change the config")
        self.assertEqual([i for i, _ in plan], [i for i in ST.APPLY_ORDER if ST.BY_ID[i].applies(ctx)])
        self.assertNotIn("desktop", [i for i, _ in plan], "the desktop stage only exists with a hardware profile that ships a desktop")
        text = "\n".join(l for _i, ls in plan for l in ls)
        self.assertIn("ERASE /dev/vda", text)
        self.assertIn("ESP", text)
        self.assertIn("key-only", text)

    def test_disk_layout_sums_to_disk(self):
        c = S.default_config(PROFILE_X)
        c["fs"], c["swap"] = "ext4", "hibernate"      # portables: a hibernate swap partition (RAM x 1.125); ZFS has no swap partition
        segs = ST.disk_layout(c, PROFILE_X, 100 * 10**9, True, 16384)
        self.assertEqual([s[0] for s in segs], ["ESP", "boot", "swap", "root"])
        self.assertLessEqual(sum(s[1] for s in segs), 100 * 10**9)
        c["swap"] = "none"                            # servers: no swap partition
        self.assertEqual([s[0] for s in ST.disk_layout(c, PROFILE_X, 100 * 10**9, True, 16384)], ["ESP", "boot", "root"])
        c["swap"] = "hibernate"
        bios = ST.disk_layout(c, PROFILE_X, 100 * 10**9, False, 8192)
        self.assertEqual(bios[0][0], "BIOS boot")

    def test_run_stages_in_apply_order(self):
        calls = []

        class Fake:
            def __init__(self):
                self.c = S.default_config(PROFILE_X)

            def ctx(self):
                return ST.Ctx(PROFILE_X, self.c)

            def log(self, l):
                pass
        for sid in ST.APPLY_ORDER:
            setattr(Fake, "stage_" + sid, lambda self, sid=sid: calls.append(sid))
        ST.run_stages(Fake())
        self.assertEqual(calls, [i for i in ST.APPLY_ORDER if ST.BY_ID[i].applies(Fake().ctx())])

    def test_form_stages_share_fields_with_validation(self):
        c = S.default_config(PROFILE_X)
        ctx = ST.Ctx(PROFILE_X, c)
        stage = ST.BY_ID["rootpass"]
        fields = stage.fields(ctx)
        for f in fields:
            if f.key in ("pw", "pw2"):
                f.value = "short"
            if f.key == "email":
                f.value = "a@example.com"
        bad = ST.first_problem(stage, ctx, fields)
        self.assertIn("at least 12", bad[1])
        for f in fields:
            if f.key in ("pw", "pw2"):
                f.value = GOOD_PW
            if f.key == "email":
                f.value = "a@example.com"
        self.assertIsNone(ST.first_problem(stage, ctx, fields))
        self.assertTrue(any(f.advanced for f in fields), "the SSH key lives under Advanced options")

    def test_services_optional_only(self):
        c = S.default_config(PROFILE_X)
        c["services_off"] = ["horcrux-api"]
        c["disks"] = ["/dev/vda"]
        c["root_hash"] = HASH
        self.assertTrue(any("optional" in e for e in S.validate_config(PROFILE_X, c)))
        c["services_off"] = ["docker"]
        self.assertEqual([e for e in S.validate_config(PROFILE_X, c) if "services" in e], [])
        plan = ST.BY_ID["services"].plan(ST.Ctx(PROFILE_X, c))
        self.assertIn("docker", plan[1])


class AnswerFileStages(unittest.TestCase):
    def cfg(self, text):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write(textwrap.dedent(text))
        try:
            return S.config_from_answers(PROFILE_X, f.name)
        finally:
            os.unlink(f.name)

    def test_new_sections(self):
        c = self.cfg(f"""
            [global]
            hostname = "a.example.com"
            root_password_hash = "{HASH}"
            install_type = "server"
            [disk]
            disks = ["/dev/vda"]
            [services]
            disable = ["docker"]
            [hardening]
            ssh = false
            sysctl = false
            [use_flags]
            preset = "minimal"
            enable = ["ssl"]
            disable = ["debug"]
        """)
        self.assertEqual(c["services_off"], ["docker"])
        self.assertFalse(c["ssh_enabled"])
        self.assertFalse(c["sysctl_hardening"])
        self.assertEqual(c["use"]["preset"], "minimal")
        self.assertEqual(c["use"]["flags"], {"ssl": True, "debug": False})

    def test_use_flags_checked_against_portage_metadata(self):
        os.environ["LIVEINST_META"] = FIXTURE
        try:
            c = self.cfg(f"""
                [global]
                hostname = "a.example.com"
                root_password_hash = "{HASH}"
                [disk]
                disks = ["/dev/vda"]
                [use_flags]
                disable = ["ssl"]
            """)
            errs = S.validate_config(PROFILE_X, c)
            self.assertTrue(any(e.startswith("use_flags:") and "ssl" in e for e in errs), errs)
            c = self.cfg(f"""
                [global]
                hostname = "a.example.com"
                root_password_hash = "{HASH}"
                [disk]
                disks = ["/dev/vda"]
                [use_flags]
                preset = "nonsense"
            """)
            self.assertTrue(any("preset" in e for e in S.validate_config(PROFILE_X, c)))
        finally:
            del os.environ["LIVEINST_META"]


class UseFlagLogic(unittest.TestCase):
    def test_required_use_parse_eval_explain(self):
        n = UF.parse_required_use("^^ ( a b ) c? ( a ) !d? ( || ( e f ) ) ?? ( g h )")
        self.assertTrue(UF.ru_eval(n, {"a", "d"}))
        self.assertFalse(UF.ru_eval(n, {"a", "b", "d"}))            # both a and b
        self.assertFalse(UF.ru_eval(n, {"b", "c"}))                 # c needs a
        self.assertFalse(UF.ru_eval(n, set()))                      # neither a nor b
        self.assertFalse(UF.ru_eval(n, {"a"}))                      # !d? needs e or f
        self.assertFalse(UF.ru_eval(n, {"a", "d", "g", "h"}))       # at most one of g h
        why = " ".join(UF.ru_explain(n, {"b", "c", "d"}))
        self.assertIn("because 'c' is on", why)
        self.assertIn("'a'", why)
        self.assertIn("exactly one of 'a', 'b'", " ".join(UF.ru_explain(n, {"d"})))

    def test_check_required_use_use_dep_and_blocker(self):
        m = tiny_meta()
        prof = {"title": "T"}
        self.assertEqual(UF.check(m, prof, {}), [])
        i = UF.check(m, prof, {"b": True})                          # a and b on: ^^ violated
        self.assertTrue(any(x.code == "required_use" and "exactly one" in x.text for x in i), i)
        i = UF.check(m, prof, {"x": False})                         # cat/A needs cat/B[x]
        self.assertTrue(any(x.code == "use_dep" and "cat/A needs cat/B built with 'x'" in x.text for x in i), i)
        i = UF.check(m, prof, {"feat": True})                       # cat/D then blocks cat/E
        self.assertTrue(any(x.code == "blocker" and "cat/E" in x.text for x in i), i)
        self.assertTrue(UF.has_errors(i))

    def test_critical_flags_are_hard_errors(self):
        m = tiny_meta()
        prof = {"title": "Horcrux", "use_critical": ["cat/B:x"], "use_critical_why": {"cat/B:x": "the product links it"}}
        i = UF.check(m, prof, {"x": False})
        crit = [x for x in i if x.code == "critical"]
        self.assertEqual(len(crit), 1)
        self.assertEqual(crit[0].level, "error")
        self.assertIn("the product links it", crit[0].text)
        self.assertEqual(UF.preset_flags(m, prof, "minimal").get("x"), None)

    def test_security_warnings(self):
        m = UF.load_meta(FIXTURE)
        off = [x for x in UF.check(m, {"title": "T"}, {"pam": False}) if x.code == "security"]
        self.assertTrue(off and off[0].level == "warning" and "pam" in off[0].text.lower())
        on = [x for x in UF.check(m, {"title": "T"}, {"debug": True}) if x.code == "security"]
        self.assertTrue(on)

    def test_impact_binary_vs_source(self):
        m = tiny_meta()
        self.assertEqual(UF.impact(m, {})["packages"], 0)
        self.assertIn("No rebuild", UF.summary_line(m, {}))
        imp = UF.impact(m, {"x": False})
        self.assertEqual({p for p, _m in imp["list"]}, {"cat/B"})
        self.assertGreater(imp["minutes"], 0)
        self.assertIn("1 of 4 packages", UF.summary_line(m, {"x": False}))

    def test_real_metadata_lists_every_flag_with_description_and_group(self):
        m = UF.load_meta(FIXTURE)
        rows = m.flag_rows()
        self.assertGreater(len(rows), 500)
        self.assertTrue(all(r["desc"] and r["group"] in UF.GROUPS for r in rows))
        self.assertTrue(any(r["flag"] == "ssl" for r in rows))
        self.assertIn("ssl", [r["flag"] for r in m.search("ssl", rows)])
        self.assertTrue(m.search("encryption"), "search matches descriptions")
        i = UF.check(m, {"title": "T"}, {"ssl": False})
        self.assertTrue(any(x.level == "error" for x in i), "ssl off breaks installed packages' [ssl] use deps")

    def test_presets_skip_critical_and_unknown(self):
        m = tiny_meta()
        prof = {"use_critical": ["x"], "use_preset_minimal_off": ["x", "nonexistent", "a"], "use_preset_full_on": ["b"]}
        self.assertEqual(UF.preset_flags(m, prof, "minimal"), {}, "'a' off would break cat/A's REQUIRED_USE (^^ ( a b )): the preset drops it")
        prof2 = dict(prof, use_preset_minimal_off=["x", "nonexistent", "c"])
        self.assertEqual(UF.preset_flags(m, prof2, "minimal"), {}, "c is off in the image already: nothing to change")
        prof3 = {"use_preset_minimal_off": ["a"], "use_preset_full_on": ["feat", "b"]}
        full = UF.preset_flags(m, prof3, "full")
        self.assertEqual([i for i in UF.check(m, prof3, full) if i.level == "error"], [], "a preset never ships a Portage error")
        self.assertEqual(UF.preset_flags(m, prof, "full"), {}, "'b' on alongside 'a' breaks ^^ ( a b ): dropped")
        self.assertEqual(UF.preset_flags(m, prof, "recommended"), {})

    def test_package_use_text(self):
        t = UF.package_use_text({"ssl": True, "cat/pkg:debug": False})
        self.assertIn("*/* ssl", t)
        self.assertIn("cat/pkg -debug", t)


class ServerHasNoSwapPartition(unittest.TestCase):
    """OWNER RULE: servers, firewalls and hypervisors never get an on-disk swap (partition, LV or file); zram is the only swap.
    Only a detected portable (battery / laptop chassis) gets a hibernate swap."""

    def _plan(self, prof, fs, portable, swap="auto", it="server"):
        from liveinst import portableswap as PS
        import liveinst.stages as stg
        c = S.default_config(prof)
        c.update(fs=fs, swap=swap, install_type=it)
        old = PS.detect_portable
        PS.detect_portable = lambda *a, **k: (portable, "test")
        try:
            return stg.swap_plan(c, prof, 8192), stg.disk_layout(c, prof, 200 * 10**9, True, 8192)
        finally:
            PS.detect_portable = old

    def test_default_config_is_auto(self):
        self.assertEqual(S.default_config(PROFILE)["swap"], "auto")

    def test_non_portable_default_is_none_for_every_filesystem_and_product(self):
        for prof in (PROFILE, FW):
            for fs in prof["filesystems"] + ["xfs-luks"]:
                p, segs = self._plan(prof, fs, portable=False)
                self.assertEqual(p["mode"], "none", (prof["name"], fs))
                self.assertNotIn("swap", [s_[0] for s_ in segs], (prof["name"], fs))
                self.assertFalse([s_ for s_ in segs if s_[2] == "swap"])

    def test_portable_gets_hibernate_only_on_the_horcrux_profile(self):
        p, segs = self._plan(PROFILE, "ext4", portable=True)
        self.assertEqual(p["mode"], "hibernate")
        self.assertIn("swap", [s_[0] for s_ in segs])
        p, segs = self._plan(dict(FW, portable_swap=False), "ext4", portable=True)      # the firewall profile opts out
        self.assertEqual(p["mode"], "none")
        self.assertNotIn("swap", [s_[0] for s_ in segs])

    def test_explicit_none_beats_a_portable(self):
        p, _ = self._plan(PROFILE, "ext4", portable=True, swap="none")
        self.assertEqual(p["mode"], "none")

    def test_sgdisk_writes_no_partition_3_without_swap(self):
        cmds = []
        inst = object.__new__(S.Installer)
        inst.c = dict(S.default_config(PROFILE), fs="ext4", disks=["/dev/vda"])
        inst.p, inst.efi, inst.target = PROFILE, True, "/mnt/target"
        inst.swap_mode, inst.swap_mb = "none", 0
        inst.run = lambda cmd, **k: cmds.append(cmd if isinstance(cmd, str) else " ".join(cmd)) or 0
        old, S.time.sleep = S.time.sleep, lambda *_: None
        try:
            inst.partition()
        finally:
            S.time.sleep = old
        sg = [c for c in cmds if c.startswith("sgdisk -n")][0]
        self.assertNotIn("8200", sg)
        self.assertNotIn("swap", sg)
        self.assertNotIn("3:0:", sg)
        self.assertIn("4:0:0", sg)                       # root stays partition 4: no gap handling anywhere else
        inst.swap_mode, inst.swap_mb = "hibernate", 4608
        cmds.clear()
        S.time.sleep = lambda *_: None
        try:
            inst.partition()
        finally:
            S.time.sleep = old
        self.assertIn("3:0:+4608M", [c for c in cmds if c.startswith("sgdisk -n")][0])

    def test_zram_service_ships_and_is_exec(self):
        base = os.path.join(HERE, "..", "overlay", "etc")
        self.assertTrue(os.access(os.path.join(base, "init.d", "zram-swap"), os.X_OK))
        self.assertTrue(os.path.exists(os.path.join(base, "conf.d", "zram-swap")))


class ShellScriptsParse(unittest.TestCase):
    """init scripts and session programs are shell: a bad merge breaks the boot, so parse every one with sh -n"""

    def test_sh_syntax(self):
        import glob
        import subprocess
        root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "overlay")
        files = glob.glob(os.path.join(root, "etc/init.d/*")) + glob.glob(os.path.join(root, "usr/libexec/live-*")) \
            + glob.glob(os.path.join(root, "etc/local.d/*"))
        for f in files:
            with open(f, errors="replace") as fh:
                first = fh.readline()
            if "sh" not in first and "openrc" not in first:
                continue
            r = subprocess.run(["sh", "-n", f], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, f"{f}: {r.stderr}")


if __name__ == "__main__":
    unittest.main()


# ---------------------------------------------------------------- hardware profiles + XFS-on-LUKS
from liveinst import hwprofile as HW  # noqa: E402
from liveinst import luksroot as LR  # noqa: E402

SURFACE_TOML = os.path.join(HERE, "..", "..", "..", "..", "gentoo", "surface", "hwprofile.toml")


@unittest.skipUnless(os.path.exists(SURFACE_TOML), "gentoo/surface lives in the Horcrux repo only (vendored common/ copy in Patronus)")
class HwProfile(unittest.TestCase):
    def profiles(self):
        import tomllib
        return [tomllib.load(open(SURFACE_TOML, "rb"))]

    def test_surface_pro_1_names(self):
        for name in ("Surface with Windows 8 Pro", "Surface Pro", "surface pro", "Surface  Pro 2"):
            p, m = HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": name}, self.profiles())
            self.assertEqual((p["id"], m["id"]), ("surface", "surface-pro-1"), name)
            self.assertIn("button.lid_init_state=open", HW.kernel_cmdline(p, m))

    def test_other_surface_generic_and_non_surface_none(self):
        p, m = HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": "Surface Laptop 4"}, self.profiles())
        self.assertEqual((p["id"], m), ("surface", None))
        self.assertEqual(HW.kernel_cmdline(p, m), [])
        self.assertEqual(HW.detect({"sys_vendor": "Microsoft Corporation", "product_name": "Virtual Machine"}, self.profiles()), (None, None))
        self.assertEqual(HW.detect({"sys_vendor": "Apple Inc.", "product_name": "Surface Pro"}, self.profiles()), (None, None))
        self.assertEqual(HW.detect({"sys_vendor": "QEMU", "product_name": "Standard PC (Q35 + ICH9, 2009)"}, self.profiles()), (None, None))

    def test_default_fs(self):
        fs = ["ext4", "xfs", "zfs", "xfs-luks"]
        self.assertEqual(HW.default_fs(fs, 3800), "xfs-luks")          # 4 GB machine
        self.assertEqual(HW.default_fs(fs, 5800), "xfs-luks")          # 6 GB
        self.assertEqual(HW.default_fs(fs, 7700), "zfs")               # "8 GB" machine: MemTotal ~7.7 GB
        self.assertEqual(HW.default_fs(fs, 64000), "zfs")
        self.assertEqual(HW.default_fs(["xfs", "ext4"], 64000), "xfs")  # Patronus image: no ZFS, XFS fallback
        p = self.profiles()[0]
        self.assertEqual(HW.default_fs(fs, 64000, p), "xfs-luks")      # a profile can force it
        self.assertEqual(HW.default_fs(["xfs", "ext4", "zfs"], 3800), "xfs")     # image without xfs-luks: plain XFS below 8 GB


class T2ZfsDesktopStages(unittest.TestCase):
    """the backend keys the T2 / encrypted-ZFS / Desktop-image work added are exposed by the shared stages (text wizard AND GTK)"""

    def _ctx(self, **c):
        cfg = S.default_config(PROFILE)
        cfg.update(c)
        return ST.Ctx(PROFILE, cfg, dry=True, efi=True, ram_mb=16000), cfg

    def test_zfs_encryption_fields_validate_and_commit(self):
        ctx, cfg = self._ctx(fs="zfs")
        st = ST.BY_ID["disk"]
        keys = [f.key for f in st.fields(ctx)]
        for k in ("enc", "pp", "pp2", "swap"):
            self.assertIn(k, keys)
        v = {"fs": "zfs", "zfs_raid": "single", "enc": True, "pp": "short", "pp2": "short"}
        self.assertTrue(st.validate(ctx, v))                       # too short
        v.update(pp="correct horse battery", pp2="different horse battery")
        self.assertIn("do not match", st.validate(ctx, v))
        v.update(pp="correct horse battery", pp2="correct horse battery", swap="auto")
        self.assertIsNone(st.validate(ctx, v))
        st.commit(ctx, v)
        self.assertTrue(cfg["zfs_encrypt"])
        self.assertEqual(cfg["zfs_passphrase"], "correct horse battery")
        st.commit(ctx, dict(v, enc=False))
        self.assertFalse(cfg["zfs_encrypt"])
        self.assertEqual(cfg["zfs_passphrase"], "")

    def test_swap_field_for_zfs_only_when_encrypted(self):
        ctx, _ = self._ctx(fs="zfs")
        f = [x for x in ST.BY_ID["disk"].fields(ctx) if x.key == "swap"][0]
        self.assertFalse(f.visible({"fs": "zfs", "enc": False}))
        self.assertTrue(f.visible({"fs": "zfs", "enc": True}))
        self.assertTrue(f.visible({"fs": "xfs"}))

    def test_t2_and_gnome_stages_only_where_they_apply(self):
        ctx, cfg = self._ctx(t2="no")
        self.assertFalse(ST.BY_ID["t2"].applies(ctx))
        cfg["t2"] = "yes"
        self.assertTrue(ST.BY_ID["t2"].applies(ctx))                 # forced (QEMU / answer file)
        ST.BY_ID["t2"].commit(ctx, {"fw": "none"})
        self.assertEqual(cfg["t2_firmware"], "none")
        self.assertFalse(ST.BY_ID["gnome"].applies(ctx))             # no Desktop image here
        st = ST.BY_ID["gnome"]
        st.commit(ctx, {"user": "ann", "full": "", "pw": "Kx9!mQv2-tle-77Zp", "pw2": "Kx9!mQv2-tle-77Zp", "auto": False})
        self.assertEqual((cfg["install_type"], cfg["user_name"], cfg["user_fullname"]), ("desktop", "ann", "ann"))


class FilesystemDefault(unittest.TestCase):
    """OWNER RULE: default = ZFS; fallback = XFS (LUKS where offered) on < 8 GB RAM / no ZFS module / image without ZFS; ext4 is never the default"""
    HX = ["zfs", "xfs-luks", "xfs", "ext4"]

    def test_zfs_on_an_8gb_amd64_server(self):
        fs, why = HW.choose_fs(self.HX, 7700, None, True)
        self.assertEqual(fs, "zfs")
        self.assertIn("ZFS", why)

    def test_xfs_luks_below_8gb_with_the_reason(self):
        fs, why = HW.choose_fs(self.HX, 3800, None, True)
        self.assertEqual(fs, "xfs-luks")
        self.assertIn("RAM", why)
        self.assertEqual(HW.choose_fs(["zfs", "xfs", "ext4"], 3800, None, True)[0], "xfs")     # no LUKS offered: plain XFS

    def test_xfs_when_zfs_is_unavailable(self):
        fs, why = HW.choose_fs(self.HX, 64000, None, False)
        self.assertEqual(fs, "xfs-luks")                      # the encrypted flavour where the profile offers it
        self.assertIn("ZFS kernel module", why)
        self.assertEqual(HW.choose_fs(["zfs", "xfs", "ext4"], 64000, None, False)[0], "xfs")

    def test_image_without_zfs_is_xfs(self):
        fs, why = HW.choose_fs(["xfs", "ext4"], 64000, None, True)
        self.assertEqual(fs, "xfs")
        self.assertIn("does not ship ZFS", why)

    def test_ext4_is_never_the_default(self):
        for fs_list in (self.HX, ["xfs", "ext4"], ["zfs", "xfs", "ext4"], ["ext4", "xfs"], ["ext4", "xfs", "zfs"]):
            for mem in (1500, 3800, 7000, 7700, 16000, 131072):
                for zok in (True, False):
                    self.assertNotEqual(HW.choose_fs(fs_list, mem, None, zok)[0], "ext4", (fs_list, mem, zok))

    def test_default_config_and_answer_file(self):
        old_env, old_mem = os.environ.get("LIVEINST_ZFS"), S.mem_mb
        try:
            os.environ["LIVEINST_ZFS"] = "1"
            S.mem_mb = lambda: 16000
            self.assertEqual(S.default_config(PROFILE)["fs"], "zfs")
            with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                f.write('[disk]\ndisks = ["/dev/vda"]\n')           # no filesystem key
            c = S.config_from_answers(PROFILE, f.name)
            self.assertEqual(c["fs"], "zfs")                       # the answer file used to default to ext4
            S.mem_mb = lambda: 3800
            self.assertEqual(S.config_from_answers(PROFILE, f.name)["fs"], "xfs")        # PROFILE has no xfs-luks
            c = S.default_config(dict(PROFILE, filesystems=self.HX))
            self.assertEqual(c["fs"], "xfs-luks")
            self.assertIn("RAM", c["fs_why"])
            os.environ["LIVEINST_ZFS"] = "0"
            S.mem_mb = lambda: 16000
            self.assertEqual(S.default_config(PROFILE)["fs"], "xfs")
            self.assertEqual(S.default_config(FW)["fs"], "xfs" if "xfs" in FW["filesystems"] else "ext4")
            with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
                f.write('[disk]\nfilesystem = "ext4"\ndisks = ["/dev/vda"]\n')
            self.assertEqual(S.config_from_answers(PROFILE, f.name)["fs"], "ext4")       # still available as a manual choice
        finally:
            S.mem_mb = old_mem
            if old_env is None:
                os.environ.pop("LIVEINST_ZFS", None)
            else:
                os.environ["LIVEINST_ZFS"] = old_env

    def test_shipped_profiles_never_preselect_ext4_and_the_disk_page_shows_the_reason(self):
        import tomllib
        for rel in ("project/overlay/etc/live-installer/profile.toml",):
            path = os.path.join(HERE, "..", "..", rel)
            if os.path.exists(path):
                fss = tomllib.load(open(path, "rb"))["filesystems"]
                self.assertNotEqual(fss[0], "ext4")
        c = dict(S.default_config(PROFILE), fs_why="XFS instead of ZFS: only 3.7 GB RAM")
        ctx = type("C", (), {"c": c, "p": PROFILE, "ram_mb": 3800})()
        f = ST.DiskStage().fields(ctx)[0]
        self.assertIn("only 3.7 GB RAM", f.hint)
        self.assertEqual(f.options[-1][0], "ext4")                    # last, never first


class LuksRoot(unittest.TestCase):
    def test_argon_memory_capped(self):
        self.assertEqual(LR.pbkdf_memory_kib(3800) // 1024, 712)
        self.assertEqual(LR.pbkdf_memory_kib(4096) // 1024, 768)
        self.assertEqual(LR.pbkdf_memory_kib(8192), 1024 * 1024)
        self.assertEqual(LR.pbkdf_memory_kib(65536), 1024 * 1024)      # cap 1 GiB
        self.assertGreaterEqual(LR.pbkdf_memory_kib(100), 64 * 1024)
        self.assertLessEqual(LR.pbkdf_memory_kib(4096) * 1024, 4096 * 1024 * 1024 // 4)   # never more than a quarter of RAM

    def test_format_cmd(self):
        c = LR.luks_format_cmd("/dev/sda3", 4096)
        self.assertIn("luks2", c)
        self.assertEqual(c[c.index("--pbkdf") + 1], "argon2id")
        self.assertEqual(c[c.index("--pbkdf-memory") + 1], str(768 * 1024))
        self.assertIn("--key-file=-", c)                                  # passphrase on stdin, never in argv
        self.assertNotIn("discard", " ".join(LR.luks_format_cmd("/dev/sda3", 4096)))
        self.assertIn("--allow-discards", LR.luks_open_cmd("/dev/sda3"))

    def test_sgdisk_layout(self):
        a = " ".join(LR.sgdisk_args("/dev/sda", True))
        self.assertIn("1:0:+1G", a)
        self.assertIn("1:ef00", a)
        self.assertIn("3:0:0", a)
        self.assertIn("3:8309", a)                                        # Linux LUKS
        self.assertNotIn("8200", a)                                       # no swap partition: zram only
        self.assertIn("1:ef02", " ".join(LR.sgdisk_args("/dev/sda", False)))

    def test_kernel_args_and_fstab(self):
        k = LR.kernel_args("1234-abcd")
        for part in ("rd.luks.uuid=1234-abcd", "rd.luks.name=1234-abcd=hxroot", "rd.luks.allow-discards=1234-abcd", "root=/dev/mapper/hxroot"):
            self.assertIn(part, k)
        f = LR.fstab_text("UUID=r", "b", "e")
        self.assertIn("/\txfs\tdefaults", f)
        self.assertNotIn("discard", f)                                    # XFS mount options: defaults only
        self.assertNotIn("swap", f)

    def test_dracut_has_keyboard_stack(self):
        d = LR.dracut_conf()
        for m in ("crypt", "usbhid", "hid_multitouch", "hid_microsoft", "xhci_pci", "i915", "dm_crypt"):
            self.assertIn(m, d)

    def test_verify_initramfs(self):
        ok = "cryptsetup\nusbhid.ko\nhid-multitouch.ko\nhid-microsoft.ko\nxhci-pci.ko\ndm-crypt.ko"
        self.assertEqual(LR.verify_initramfs(ok), [])
        self.assertEqual(LR.verify_initramfs("cryptsetup\nusbhid.ko"), ["dm-crypt", "xhci", "hid-multitouch", "hid-microsoft"])
        # Gentoo dist kernel: usbhid / xhci-pci built in, dm-crypt + hid_* are modules
        cfg = "CONFIG_USB_HID=y\nCONFIG_USB_XHCI_PCI=y\nCONFIG_DM_CRYPT=m\n"
        lst = "cryptsetup\ndm-crypt.ko\nhid-multitouch.ko\nhid-microsoft.ko"
        self.assertEqual(LR.verify_initramfs(lst, cfg), [])
        self.assertEqual(LR.verify_initramfs("cryptsetup\nhid-microsoft.ko", cfg), ["dm-crypt", "hid-multitouch"])

    def test_validate(self):
        c = {"fs": "xfs-luks", "luks_passphrase": "short", "disks": ["/dev/sda"]}
        self.assertTrue(LR.validate(c))
        c["luks_passphrase"] = GOOD_PW
        self.assertEqual(LR.validate(c), [])
        self.assertEqual(LR.validate({"fs": "ext4"}), [])

    def test_config_defaults_and_answers(self):
        prof = dict(PROFILE, filesystems=["ext4", "xfs", "zfs", "xfs-luks"])
        c = S.default_config(prof)
        self.assertIn(c["fs"], ("zfs", "xfs-luks"))
        self.assertEqual(c["luks_passphrase"], "")
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[global]\nroot_password_hash = "%s"\n[disk]\nfilesystem = "xfs-luks"\nluks_passphrase = "%s"\ndisks = ["/dev/vda"]\n'
                    '[desktop]\nenable = true\nuser = "surface"\nautologin = true\n[hardware]\nprofile = "surface"\n' % (HASH, GOOD_PW))
        c = S.config_from_answers(prof, f.name)
        os.unlink(f.name)
        self.assertEqual((c["fs"], c["luks_passphrase"], c["hw"]), ("xfs-luks", GOOD_PW, "surface"))
        self.assertTrue(c["desktop"]["enable"] and c["desktop"]["autologin"])
        self.assertEqual(S.validate_config(prof, c), [])
        c["luks_passphrase"] = "x"
        self.assertTrue(any("LUKS" in e for e in S.validate_config(prof, c)))


class DracutDrivers(unittest.TestCase):
    DEP = ("kernel/drivers/hid/hid-multitouch.ko: kernel/drivers/hid/hid.ko\nkernel/drivers/hid/hid-microsoft.ko:\n"
           "kernel/drivers/md/dm-crypt.ko:\nkernel/drivers/gpu/drm/i915/i915.ko.zst: x\n")

    def test_filter_drivers_drops_missing_and_builtin(self):
        out = LR.filter_drivers("usbhid hid_multitouch hid_microsoft sha256_ssse3 dm_crypt i915 xfs", self.DEP)
        self.assertEqual(out, ["hid_multitouch", "hid_microsoft", "dm_crypt", "i915"])   # usbhid/xfs are built in; sha256_ssse3 does not exist

    def test_dracut_conf_uses_filtered_list(self):
        self.assertIn('add_drivers+=" dm_crypt "', LR.dracut_conf("dm_crypt"))
