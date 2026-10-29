"""Unit tests for the boot menu generator and the rescue component (pure logic; no disks touched).
Run: python3 -m unittest discover -s build/live-iso/common/tests   (Python >= 3.11)
"""
import io
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
sys.path.insert(0, os.path.join(COMMON, "overlay", "usr", "lib", "live-installer"))
from rescue import scan as SC  # noqa: E402
from rescue import ops as OPS  # noqa: E402
from rescue.menu import Menu  # noqa: E402

GEN = os.path.join(COMMON, "boot", "gen-grub-cfg.sh")
PROFILE = {"name": "horcrux", "title": "Horcrux", "db_path": "/var/lib/horcrux/horcrux.db", "db_table": "users",
           "admin_password_file": "/etc/horcrux/admin-password", "admin_password_owner": "root",
           "config_paths": ["/etc/horcrux", "/var/lib/horcrux/horcrux.db"], "factory_reset_files": ["/var/lib/horcrux/initial-admin-password"]}

LSBLK = json.dumps({"blockdevices": [
    {"name": "/dev/vda", "path": "/dev/vda", "fstype": None, "label": None, "uuid": None, "size": "26843545600", "type": "disk", "mountpoint": None, "pkname": None, "rm": False, "partlabel": None, "ro": False,
     "children": [
         {"name": "/dev/vda1", "path": "/dev/vda1", "fstype": "vfat", "label": "ESP", "uuid": "AAAA-BBBB", "size": "536870912", "type": "part", "mountpoint": None, "pkname": "/dev/vda", "rm": False, "partlabel": "ESP", "ro": False},
         {"name": "/dev/vda4", "path": "/dev/vda4", "fstype": "ext4", "label": "root", "uuid": "u-4", "size": "20000000000", "type": "part", "mountpoint": None, "pkname": "/dev/vda", "rm": False, "partlabel": "root", "ro": False},
         {"name": "/dev/vda5", "path": "/dev/vda5", "fstype": "crypto_LUKS", "label": None, "uuid": "u-5", "size": "1000", "type": "part", "mountpoint": None, "pkname": "/dev/vda", "rm": False, "partlabel": None, "ro": False}]},
    {"name": "/dev/sda", "path": "/dev/sda", "fstype": None, "label": None, "uuid": None, "size": "2000000000", "type": "disk", "mountpoint": None, "pkname": None, "rm": True, "partlabel": None, "ro": False,
     "children": [{"name": "/dev/sda1", "path": "/dev/sda1", "fstype": "iso9660", "label": "HORCRUX_LIVE", "uuid": "x", "size": "1900000000", "type": "part", "mountpoint": "/run/initramfs/live", "pkname": "/dev/sda", "rm": True, "partlabel": None, "ro": True}]},
]})


def gen(arch="x86_64", memtest="both"):
    return subprocess.run(["bash", GEN, "HORCRUX_LIVE", "Horcrux", "horcrux", arch, memtest], capture_output=True, text=True, check=True).stdout


class BootMenu(unittest.TestCase):
    def test_all_entries_in_order_with_hotkeys(self):
        cfg = gen()
        top = cfg.split("submenu")[0]
        titles = re.findall(r'menuentry "Horcrux - ([^"]+)" --hotkey=(\w)', top)
        self.assertEqual([k for _, k in titles][:9], list("123456789"))
        self.assertIn("0", [k for _, k in titles])            # memory test
        names = [t for t, _ in titles]
        for want in ("Graphical install", "Install (text mode)", "Advanced (expert) install", "Automated install", "Rescue mode",
                     "Rescue (forensic, no-touch", "Try it live (no", "Try it live with persistence", "Safe graphics (nomodeset)", "Memory test"):
            self.assertTrue(any(n.startswith(want) for n in names), want)
        self.assertTrue(names[0].startswith("Graphical install"))
        self.assertIn("set default=0", cfg)

    def test_modes_on_the_kernel_line(self):
        cfg = gen()
        for flag in ("horcrux.installer=gui", "horcrux.installer=1", "horcrux.installer=expert", "horcrux.autoinstall=1",
                     "horcrux.rescue=1", "horcrux.rescue=forensic", "horcrux.live=1", "horcrux.persist=1", "nomodeset"):
            self.assertIn(flag, cfg)
        self.assertIn("rd.md=0 rd.lvm=0", cfg)        # forensic: no RAID/LVM assembly in the initramfs

    def test_serial_submenu_repeats_everything_on_the_uart(self):
        cfg = gen()
        sub = cfg.split("submenu", 1)[1]
        self.assertEqual(sub.count("console=ttyS0,115200n8") >= 9, True)
        self.assertEqual(len(re.findall(r"menuentry ", sub)), 9)

    def test_same_menu_on_vga_and_serial(self):
        cfg = gen()
        self.assertIn("terminal_output $gfxout serial", cfg)       # mirrored on COM1 (text even when the screen shows the splash)
        self.assertIn("terminal_input console serial", cfg)
        self.assertNotIn("gfxterm", cfg)                           # one text menu for both outputs

    def test_memtest_x86_only_and_per_firmware(self):
        self.assertIn("linux16 /boot/memtest/memtest.bin", gen(memtest="both"))
        self.assertIn("chainloader /boot/memtest/memtest.efi", gen(memtest="both"))
        self.assertNotIn("memtest.efi", gen(memtest="bios"))
        self.assertNotIn("Memory test", gen(memtest="none"))
        self.assertNotIn("Memory test", gen(arch="aarch64", memtest="both"))

    def test_other_arch_has_no_bios_serial_probe_and_its_own_uart(self):
        cfg = gen(arch="aarch64", memtest="none")
        self.assertNotIn("serial --unit", cfg)
        self.assertIn("console=ttyAMA0,115200n8", cfg)

    def test_splash_block_only_when_the_branding_files_are_on_the_iso(self):
        plain = gen()
        self.assertNotIn("gfxterm", plain)
        gfx = subprocess.run(["bash", GEN, "HORCRUX_LIVE", "Horcrux", "horcrux", "x86_64", "none"], capture_output=True, text=True, check=True,
                             env=dict(os.environ, GFX_OK="1")).stdout
        self.assertIn("background_image /boot/grub/splash.png", gfx)
        self.assertIn("terminal_output $gfxout serial", gfx)        # the serial mirror stays text
        self.assertEqual(gfx.count("{"), gfx.count("}"))

    def test_no_gfxterm_on_arm64_where_the_uart_is_the_efi_console(self):
        cfg = subprocess.run(["bash", GEN, "HORCRUX_LIVE", "Horcrux", "horcrux", "aarch64", "none"], capture_output=True, text=True, check=True,
                             env=dict(os.environ, GFX_OK="1")).stdout
        self.assertNotIn("gfxterm", cfg)      # gfxterm output never reaches the UART: serial consoles would see no menu
        self.assertIn("Graphical install", cfg)

    def test_valid_grub_syntax_balanced(self):
        cfg = gen()
        self.assertEqual(cfg.count("{"), cfg.count("}"))


class Scan(unittest.TestCase):
    def test_lsblk_and_live_medium_hidden(self):
        devs = SC.parse_lsblk(LSBLK)
        paths = [d["path"] for d in devs]
        self.assertIn("/dev/vda4", paths)
        live = SC.live_medium_devices(devs, "HORCRUX_LIVE")
        self.assertEqual(live, {"/dev/sda", "/dev/sda1"})
        self.assertEqual([d["path"] for d in devs if d["path"] not in live and SC.classify(d) == "fs"], ["/dev/vda4"])
        self.assertEqual([d["path"] for d in devs if SC.classify(d) == "locked"], ["/dev/vda5"])

    def test_lsblk_garbage(self):
        self.assertEqual(SC.parse_lsblk("not json"), [])

    def test_zpool_import_listing(self):
        txt = "   pool: rpool\n     id: 1234567890\n  state: ONLINE\n action: ok\n config:\n\n\trpool  ONLINE\n   pool: tank\n     id: 55\n  state: DEGRADED\n"
        self.assertEqual(SC.parse_zpool_import(txt), [{"name": "rpool", "id": "1234567890", "state": "ONLINE"}, {"name": "tank", "id": "55", "state": "DEGRADED"}])

    def test_zfs_root_datasets(self):
        out = "rpool\t/mnt/rescue/zfs-rpool\toff\tfilesystem\nrpool/ROOT\t/mnt/rescue/zfs-rpool\tnoauto\tfilesystem\nrpool/data\t/mnt/rescue/zfs-rpool/data\ton\tfilesystem\n"
        self.assertEqual(SC.zfs_root_datasets(out, "/mnt/rescue/zfs-rpool"), ["rpool/ROOT"])
        self.assertEqual(SC.zfs_child_datasets(out, "/mnt/rescue/zfs-rpool"), ["rpool/data"])

    def test_fstab_never_swap_or_noauto(self):
        fstab = "UUID=a / ext4 defaults 0 1\nUUID=b /boot ext4 defaults 0 2\nUUID=c /boot/efi vfat umask=0077 0 2\nUUID=d none swap sw 0 0\n" \
                "/dev/vdb /data ext4 noauto 0 0\nserver:/x /mnt nfs defaults 0 0\ntmpfs /tmp tmpfs defaults 0 0\n"
        self.assertEqual([m[1] for m in SC.fstab_mounts(fstab)], ["/boot", "/boot/efi"])

    def test_blkid_args(self):
        self.assertEqual(SC.blkid_args("UUID=abc"), ["blkid", "-U", "abc"])
        self.assertEqual(SC.blkid_args("LABEL=boot"), ["blkid", "-L", "boot"])
        self.assertIsNone(SC.blkid_args("/dev/vda2"))

    def test_forensic_mount_options_never_replay(self):
        self.assertIn("noload", SC.ro_mount_opts("ext4", True))
        self.assertIn("norecovery", SC.ro_mount_opts("xfs", True))
        self.assertTrue(SC.ro_mount_opts("ext4", True).startswith("ro"))
        self.assertEqual(SC.ro_mount_opts("ext4", False), "rw")
        self.assertTrue(SC.probe_mount_opts("ext4").startswith("ro"))

    def test_grub_targets_per_arch(self):
        self.assertEqual(SC.grub_target("x86_64", True), "x86_64-efi")
        self.assertEqual(SC.grub_target("x86_64", False), "i386-pc")
        self.assertEqual(SC.grub_target("aarch64", True), "arm64-efi")
        self.assertEqual(SC.grub_target("riscv64", True), "riscv64-efi")
        self.assertIsNone(SC.grub_target("aarch64", False))        # BIOS exists on x86 only

    def test_boot_parts_found_by_label_without_fstab(self):
        devs = SC.parse_lsblk(LSBLK) + [
            {"name": "x", "path": "/dev/vda2", "fstype": "ext4", "label": "boot", "uuid": "b", "size": 1, "type": "part", "mountpoint": None, "parent": "/dev/vda", "partlabel": "boot"},
            {"name": "y", "path": "/dev/vdb2", "fstype": "ext4", "label": "boot", "uuid": "c", "size": 1, "type": "part", "mountpoint": None, "parent": "/dev/vdb", "partlabel": "boot"}]
        boot, esp = SC.pick_boot_parts(devs, "/dev/vda4", live={"/dev/sda", "/dev/sda1"})
        self.assertEqual((boot, esp), ("/dev/vda2", "/dev/vda1"))           # same disk as the root wins

    def test_static_zfs_grubcfg(self):
        c = SC.static_zfs_grubcfg("Horcrux", "6.12.1", "uuid-1", "rpool/ROOT", "")
        self.assertIn("root=ZFS=rpool/ROOT zfsforce=1", c)
        self.assertNotIn("serial", c)

    def test_os_pretty(self):
        self.assertEqual(SC.os_pretty('NAME="Horcrux"\nPRETTY_NAME="Horcrux (Gentoo)"\n'), "Horcrux (Gentoo)")

    def test_twofactor_columns(self):
        self.assertEqual(SC.twofactor_columns(["id", "username", "totp_secret", "totp_enabled", "created"]), ["totp_secret", "totp_enabled"])


def fake_hash(pw):
    return "$argon2id$v=19$m=16,t=2,p=1$c2FsdA$" + pw


class AdminPassword(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        os.makedirs(self.root + "/var/lib/horcrux")
        self.db = self.root + "/var/lib/horcrux/horcrux.db"

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def mkdb(self, admin=True):
        con = sqlite3.connect(self.db)
        con.execute("CREATE TABLE users(id INTEGER PRIMARY KEY, username TEXT, password_hash TEXT, totp_secret TEXT, totp_enabled INTEGER DEFAULT 0)")
        con.execute("CREATE TABLE sessions(id TEXT)")
        con.execute("INSERT INTO sessions VALUES('x')")
        if admin:
            con.execute("INSERT INTO users(username,password_hash,totp_secret,totp_enabled) VALUES('admin','old','SECRET',1)")
        con.commit()
        con.close()

    def test_updates_admin_and_drops_sessions(self):
        self.mkdb()
        self.assertEqual(OPS.set_admin_password(PROFILE, self.root, "New-pw-123456", hasher=fake_hash), "db")
        con = sqlite3.connect(self.db)
        self.assertTrue(con.execute("SELECT password_hash FROM users WHERE username='admin'").fetchone()[0].endswith("New-pw-123456"))
        self.assertEqual(con.execute("SELECT count(*) FROM sessions").fetchone()[0], 0)

    def test_without_account_writes_firstboot_file_0600(self):
        self.mkdb(admin=False)
        self.assertEqual(OPS.set_admin_password(PROFILE, self.root, "New-pw-123456", hasher=fake_hash), "file")
        p = self.root + "/etc/horcrux/admin-password"
        self.assertEqual(open(p).read().strip(), "New-pw-123456")
        self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)

    def test_no_db_at_all_writes_firstboot_file(self):
        self.assertEqual(OPS.set_admin_password(PROFILE, self.root, "New-pw-123456", hasher=fake_hash), "file")

    def test_twofactor_detect_and_clear(self):
        self.mkdb()
        self.assertEqual(dict(OPS.admin_twofactor_state(PROFILE, self.root)), {"totp_secret": "SECRET", "totp_enabled": 1})
        self.assertTrue(OPS.clear_admin_twofactor(PROFILE, self.root))
        self.assertEqual(dict(OPS.admin_twofactor_state(PROFILE, self.root)), {"totp_secret": None, "totp_enabled": 0})

    @unittest.skipUnless(shutil.which("argon2"), "argon2 CLI not installed")
    def test_real_argon2id_hash(self):
        self.mkdb()
        OPS.set_admin_password(PROFILE, self.root, "New-pw-123456")
        h = sqlite3.connect(self.db).execute("SELECT password_hash FROM users").fetchone()[0]
        self.assertTrue(h.startswith("$argon2id$"))


class FakeRun:
    """records commands, answers from a table"""
    def __init__(self, table=None):
        self.cmds, self.table = [], table or {}

    def __call__(self, cmd, timeout=0):
        key = " ".join(cmd) if isinstance(cmd, list) else cmd
        self.cmds.append(key)
        for k, v in self.table.items():
            if key.startswith(k):
                return v
        return 0, ""


class RescueOps(unittest.TestCase):
    def mk(self, forensic, table=None):
        run = FakeRun(table)
        r = OPS.Rescue(PROFILE, forensic, base=tempfile.mkdtemp(), run=run, tty=lambda c: (run.cmds.append(" ".join(c)) or 0), say=lambda s: None, cmdline="")
        return r, run

    def test_forensic_refuses_every_write_action(self):
        r, _ = self.mk(True)
        rec = {"kind": "fs", "dev": "/dev/vda4", "fstype": "ext4", "root": "/x", "pretty": "", "uuid": ""}
        for fn in (lambda: r.reset_password(rec, "pw"), lambda: r.restore_backup(rec, "/x.tgz"), lambda: r.reinstall_bootloader(rec)):
            with self.assertRaises(OPS.RescueError):
                fn()

    def test_forensic_mount_is_ro_noload_and_zfs_import_readonly(self):
        r, run = self.mk(True)
        rec = {"kind": "fs", "dev": "/dev/vda4", "fstype": "ext4", "root": "", "pretty": "", "uuid": ""}
        r.mount_system(rec)
        m = [c for c in run.cmds if c.startswith("mount ")][0]
        self.assertIn("-o ro,noload,noatime /dev/vda4", m)
        r.import_pool("rpool")
        z = [c for c in run.cmds if c.startswith("zpool import")][0]
        self.assertIn("-o readonly=on", z)
        self.assertIn("-N", z)                         # imported without mounting anything

    def test_normal_mount_rw_and_luks_open_readonly_only_in_forensic(self):
        r, run = self.mk(False)
        r.mount_system({"kind": "fs", "dev": "/dev/vda4", "fstype": "ext4", "root": "", "pretty": "", "uuid": ""})
        self.assertIn("-o rw /dev/vda4", [c for c in run.cmds if c.startswith("mount ")][0])
        r.unlock("/dev/vda5")
        self.assertNotIn("--readonly", run.cmds[-2] if len(run.cmds) > 1 else "")
        rf, runf = self.mk(True)
        rf.unlock("/dev/vda5")
        self.assertTrue(any("cryptsetup open --readonly /dev/vda5" in c for c in runf.cmds))

    def test_never_swapon(self):
        r, run = self.mk(False)
        root = tempfile.mkdtemp()
        os.makedirs(root + "/etc")
        open(root + "/etc/fstab", "w").write("UUID=s none swap sw 0 0\n")
        r.mount_fstab(root)
        self.assertFalse(any("swapon" in c for c in run.cmds))

    def test_restore_rejects_foreign_backup(self):
        r, _ = self.mk(False)
        import tarfile
        d = tempfile.mkdtemp()
        p = d + "/b.tar.gz"
        with tarfile.open(p, "w:gz") as tf:
            m = tarfile.TarInfo("MANIFEST.json")
            data = json.dumps({"project": "patronus"}).encode()
            m.size = len(data)
            tf.addfile(m, io.BytesIO(data))
        ok, msg = r.check_backup(p)
        self.assertFalse(ok)
        self.assertIn("patronus", msg)

    def test_restore_extracts_into_root_and_snapshots(self):
        r, _ = self.mk(False)
        import tarfile
        root = tempfile.mkdtemp()
        os.makedirs(root + "/etc/horcrux")
        open(root + "/etc/horcrux/old.conf", "w").write("old")
        d = tempfile.mkdtemp()
        p = d + "/b.tar.gz"
        with tarfile.open(p, "w:gz") as tf:
            for name, data in (("MANIFEST.json", json.dumps({"project": "horcrux"}).encode()), ("etc/horcrux/new.conf", b"new")):
                ti = tarfile.TarInfo(name)
                ti.size = len(data)
                tf.addfile(ti, io.BytesIO(data))
        snap = r.restore_backup({"kind": "fs", "root": root}, p)
        self.assertEqual(open(root + "/etc/horcrux/new.conf").read(), "new")
        self.assertTrue(os.path.exists(snap))


class MenuFlow(unittest.TestCase):
    def test_forensic_menu_offers_only_readonly_tools(self):
        out = io.StringIO()
        r = OPS.Rescue(PROFILE, True, base=tempfile.mkdtemp(), run=FakeRun(), say=lambda s: None)
        m = Menu(PROFILE, True, inp=lambda p: "0", out=out, rescue=r)
        txt = m.menu_text()
        self.assertIn("read-only", txt.lower())
        for bad in ("Reset the admin password", "Restore", "Reinstall the boot loader"):
            self.assertNotIn(bad, txt)
        self.assertEqual(m.loop(), 0)
        self.assertIn("FORENSIC", out.getvalue())
        self.assertFalse(any(c.startswith("mount ") for c in r.run.cmds))      # the session start mounted nothing

    def test_normal_menu_lists_the_five_actions(self):
        r = OPS.Rescue(PROFILE, False, base=tempfile.mkdtemp(), run=FakeRun(), say=lambda s: None)
        txt = Menu(PROFILE, False, inp=lambda p: "0", out=io.StringIO(), rescue=r).menu_text()
        for want in ("Shell in the installed system", "Reset the admin password", "Restore a configuration backup", "Reinstall the boot loader", "File system check"):
            self.assertIn(want, txt)


class InitScripts(unittest.TestCase):
    def read(self, rel):
        return open(os.path.join(COMMON, "overlay", rel)).read()

    def test_forensic_service_sets_ro_masks_raid_lvm_and_swap(self):
        s = self.read("etc/init.d/live-forensic")
        self.assertIn("blockdev --setro", s)
        self.assertIn("swapoff -a", s)
        self.assertIn("64-md-raid-assembly.rules", s)
        self.assertIn("69-dm-lvm.rules", s)
        self.assertIn("before udev", s)

    def test_executable_bits(self):
        for rel in ("etc/init.d/live-forensic", "etc/init.d/live-persist", "etc/init.d/live-product", "usr/bin/live-rescue", "usr/bin/live-persist",
                    "usr/libexec/live-rescue-session", "usr/libexec/live-session", "usr/libexec/live-installer-session"):
            self.assertTrue(os.access(os.path.join(COMMON, "overlay", rel), os.X_OK), rel)


if __name__ == "__main__":
    unittest.main()
