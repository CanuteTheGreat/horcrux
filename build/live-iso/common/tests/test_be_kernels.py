"""horcrux-be keeps the previous known-good kernel bootable: menu entries, retention (KERNELS_KEEP), one-shot trial of a new kernel with
fallback to the known-good one, mark-good after a successful boot, boot-previous (pure logic: fake zfs/grub-editenv, scaled-down sizes)."""
import os
import re
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import test_be_boot_space as T  # noqa: E402

BE = T.BE


def entries(text):
    return re.findall(r'menuentry "([^"]+)" --id (\S+)', text)


class Kernels(T.Boot):
    def be_dir(self, *p):
        return os.path.join(self.boot, "be", "horcrux", *p)

    def grub(self, **kw):
        with mock.patch.object(BE, "boot_uuid", lambda b=None: "1111-2222"), mock.patch.object(BE, "be_datasets", lambda pool: ["horcrux"]):
            return BE.grubcfg(self.c, self.boot)

    def install(self, tag, run=None):
        """installkernel + dracut put a new pair in /boot (older pairs stay, as on a real system), then the kernel hook runs `sync`"""
        kv = f"7.2.{tag}"
        os.makedirs(os.path.join(self.mods, kv), exist_ok=True)
        for p, n in ((f"vmlinuz-{kv}", 1), (f"initramfs-{kv}.img", 1)):
            with open(os.path.join(self.boot, p), "wb") as f:
                f.write(os.urandom(1024) * (n * 1024) + kv.encode() + p.encode())
        if run:
            self.running = run
        with mock.patch.object(BE, "boot_uuid", lambda b=None: "1111-2222"), mock.patch.object(BE, "be_datasets", lambda pool: ["horcrux"]):
            BE.cmd_sync(self.c, self.boot)
        return kv

    def boot_ok(self, kv):
        self.running = kv
        with mock.patch.object(BE, "boot_uuid", lambda b=None: "1111-2222"), mock.patch.object(BE, "be_datasets", lambda pool: ["horcrux"]):
            BE.cmd_mark_good(self.c, self.boot)

    def test_two_kernels_both_in_the_menu_and_previous_is_a_real_entry(self):
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        cfg = self.grub()
        ent = entries(cfg)
        self.assertEqual([i for _t, i in ent], ["be-horcrux", "be-horcrux-k-7.2.1"])
        self.assertIn("previous kernel", ent[1][0])
        self.assertIn("/be/horcrux/kernels/7.2.1/vmlinuz", cfg)
        self.assertIn("/be/horcrux/vmlinuz", cfg)
        for k in ("vmlinuz", "initramfs.img", "kver"):
            self.assertTrue(os.path.exists(self.be_dir("kernels", "7.2.1", k)), k)

    def test_new_kernel_is_a_one_shot_trial_old_stays_default(self):
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        cfg = self.grub()
        self.assertIn('set default="be-horcrux-k-7.2.1"', cfg)
        self.assertEqual(self.z.env.get("next_entry"), "be-horcrux")
        self.assertIn("load_env", cfg)
        self.assertIn("save_env next_entry", cfg)
        # a crash of the trial boot: GRUB cleared next_entry itself, the next boot is the known-good kernel again
        self.assertEqual(open(self.be_dir("good")).read().strip(), "7.2.1")

    def test_successful_boot_promotes_the_new_kernel(self):
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        self.boot_ok("7.2.2")
        self.assertEqual(open(self.be_dir("good")).read().strip(), "7.2.2")
        cfg = self.grub()
        self.assertIn('set default="be-horcrux"', cfg)
        self.assertNotIn("next_entry", self.z.env)
        self.assertIn("known good", [t for t, i in entries(cfg) if i == "be-horcrux"][0])

    def test_existing_install_migrates_running_kernel_is_good_and_old_loose_kernel_is_kept(self):
        # an install from before kernel history existed: the BE holds nothing historic; /boot has the older pair next to the new one; the older kernel is running
        os.makedirs(os.path.join(self.mods, "7.2.1"))
        for kv in ("7.2.1",):
            open(os.path.join(self.boot, f"vmlinuz-{kv}"), "wb").write(b"A" * 100000)
            open(os.path.join(self.boot, f"initramfs-{kv}.img"), "wb").write(b"B" * 100000)
        self.running = "7.2.1"
        self.install("2")  # the update: sync runs while 7.2.1 is still the running kernel
        self.assertEqual(open(self.be_dir("good")).read().strip(), "7.2.1")
        ids = [i for _t, i in entries(self.grub())]
        self.assertEqual(ids, ["be-horcrux", "be-horcrux-k-7.2.1"])
        self.assertEqual(self.z.env.get("next_entry"), "be-horcrux")

    def test_keep_n_prunes_the_oldest_but_never_the_known_good(self):
        self.c["KERNELS_KEEP"] = "2"
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        for t in "234":
            self.install(t)
        left = sorted(os.listdir(self.be_dir("kernels")))
        self.assertIn("7.2.1", left, "the known-good kernel survives although older than the newest N")
        self.assertIn("7.2.3", left)
        self.assertNotIn("7.2.2", left)
        self.c["KERNELS_KEEP"] = "3"
        self.boot_ok("7.2.4")
        self.install("5")
        self.assertEqual(sorted(os.listdir(self.be_dir("kernels"))), ["7.2.3", "7.2.4"], "good moved to 7.2.4: 7.2.1 is no longer protected")

    def test_keep_is_configurable_and_one_means_no_history(self):
        self.c["KERNELS_KEEP"] = "1"
        self.install("1", run="7.2.1")
        self.install("2")
        self.assertIn("be-horcrux-k-7.2.1", [i for _t, i in entries(self.grub())], "the known-good kernel stays while the new one is on trial")
        self.boot_ok("7.2.2")
        self.install("3")
        self.boot_ok("7.2.3")
        self.install("3")  # the same kernel again: a plain re-sync
        self.assertEqual([i for _t, i in entries(self.grub())], ["be-horcrux"])
        self.assertFalse(os.path.isdir(self.be_dir("kernels")) and os.listdir(self.be_dir("kernels")))

    def test_previous_kernel_costs_no_extra_space_beyond_its_own_files(self):
        self.install("1", run="7.2.1")
        self.install("2")
        objs = len(self.objects())
        self.assertEqual(objs, 4)  # 2 kernels x (vmlinuz + initramfs), each stored once
        self.snap("pre-upgrade-20261105-100000")
        self.assertEqual(len(self.objects()), 4)

    def test_no_trial_without_the_mark_good_service(self):
        self.z.good_service = False  # nothing would ever promote the new kernel: the newest must simply be the default
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        self.assertIn('set default="be-horcrux"', self.grub())
        self.assertNotIn("next_entry", self.z.env)

    def test_resync_of_the_same_kernel_needs_no_room(self):
        self.install("1", run="7.2.1")
        with mock.patch.object(BE, "space_ok", lambda *a, **k: False), mock.patch.object(BE, "say") as say:
            self.install("1")
        self.assertFalse([c for c in say.call_args_list if "no room" in str(c)])

    def test_trial_can_be_switched_off(self):
        self.c["KERNEL_TRIAL"] = "no"
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        self.assertIn('set default="be-horcrux"', self.grub())
        self.assertNotIn("next_entry", self.z.env)
        self.assertEqual([i for _t, i in entries(self.grub())], ["be-horcrux", "be-horcrux-k-7.2.1"])

    def test_unhealthy_pool_is_not_a_successful_boot(self):
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2")
        self.z.pool_health = "FAULTED"
        self.boot_ok("7.2.2")
        self.assertEqual(open(self.be_dir("good")).read().strip(), "7.2.1")

    def test_boot_previous_sets_a_one_shot_entry(self):
        self.install("1", run="7.2.1")
        self.boot_ok("7.2.1")
        self.install("2", run="7.2.2")
        self.boot_ok("7.2.2")
        BE.cmd_boot_previous(self.c, self.boot)
        self.assertEqual(self.z.env["next_entry"], "be-horcrux-k-7.2.1")

    def test_loose_old_backup_files_are_not_kernels(self):
        self.install("1", run="7.2.1")
        open(os.path.join(self.boot, "vmlinuz-7.2.0.old"), "wb").write(b"x")
        open(os.path.join(self.boot, "initramfs-7.2.0.old.img"), "wb").write(b"x")
        self.assertEqual([k[0] for k in BE.loose_kernels(self.boot)], ["7.2.1"])

    def test_version_sort_handles_suffixes(self):
        v = ["6.18.63-gentoo-dist", "6.18.64-p1-gentoo-dist", "6.18.9-gentoo-dist", "6.2.1"]
        self.assertEqual(sorted(v, key=BE.vkey, reverse=True)[:2], ["6.18.64-p1-gentoo-dist", "6.18.63-gentoo-dist"])

    def test_other_boot_environments_stay_listed(self):
        self.install("1", run="7.2.1")
        text = BE.render_grub(["horcrux", "old"], "horcrux", "rpool", "1111-2222", kernels={"horcrux": "7.2.1", "old": "7.1.9"},
                              previous={"horcrux": ["7.2.0"]}, good={"horcrux": "7.2.1"})
        self.assertEqual([i for _t, i in entries(text)], ["be-horcrux", "be-horcrux-k-7.2.0", "be-old"])


REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", ".."))
PKG = os.path.join(REPO, "gentoo", "sys-apps", "horcrux-be")


@unittest.skipUnless(os.path.isdir(PKG), "Horcrux repo tree only (not the Patronus copy)")
class Packaging(unittest.TestCase):
    """an installed machine gets the tool from sys-apps/horcrux-be, an updatable package: its copy must be the installer's copy"""

    def test_package_copy_equals_installer_copy(self):
        a = open(os.path.join(PKG, "files", "be.py"), "rb").read()
        b = open(os.path.join(T.HERE, "..", "overlay", "usr", "lib", "live-installer", "liveinst", "be.py"), "rb").read()
        self.assertEqual(a, b, "gentoo/sys-apps/horcrux-be/files/be.py must be byte-identical to liveinst/be.py")

    def test_both_inits_ship_the_known_good_service(self):
        e = open(os.path.join(PKG, "horcrux-be-1.ebuild")).read()
        self.assertIn('newinitd "${FILESDIR}"/horcrux-be-good.initd horcrux-be-good', e)
        self.assertIn('systemd_dounit "${FILESDIR}"/horcrux-be-good.service', e)
        self.assertIn("openrc-run", open(os.path.join(PKG, "files", "horcrux-be-good.initd")).read())
        self.assertIn("mark-good", open(os.path.join(PKG, "files", "horcrux-be-good.service")).read())

    def test_core_depends_on_it_and_tools_no_longer_ships_the_wrapper(self):
        core = sorted(f for f in os.listdir(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core")) if f.endswith(".ebuild"))
        self.assertIn("sys-apps/horcrux-be", open(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-core", core[-1])).read())
        tools = open(os.path.join(REPO, "gentoo", "sys-apps", "horcrux-tools", "horcrux-tools-1.ebuild")).read()
        self.assertIn("sys-apps/horcrux-be", tools)
        self.assertIn('rm -f "${ED}"/usr/bin/horcrux-be', tools)

    def test_installer_enables_the_service_and_writes_the_defaults(self):
        z = open(os.path.join(T.HERE, "..", "overlay", "usr", "lib", "live-installer", "liveinst", "zfsroot.py")).read()
        self.assertIn('initsys.enable(inst, "horcrux-be-good", "default")', z)
        for k in ("KERNELS_KEEP", "KERNEL_TRIAL", "GOOD_DELAY"):
            self.assertIn(k + "=", z)


if __name__ == "__main__":
    unittest.main()
