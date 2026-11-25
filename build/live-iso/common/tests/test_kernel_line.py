"""Installer kernel line (liveinst/kernelline.py): answer file key, validation, the stage, and the exact pin text the manager
(horcrux-api api/system_admin/kernel_line.rs, test `pin_text_matches_the_installer`) writes, so the manager adopts the installer's files.
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import json
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
LI = os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer")
sys.path.insert(0, LI)
from liveinst import kernelline as KL  # noqa: E402
from liveinst import stages as ST  # noqa: E402
from liveinst import system as S  # noqa: E402

# the same text is asserted in horcrux-api (kernel_line.rs pin_text_matches_the_installer): change both together
GOLDEN_KW = ("# Written by Horcrux: system.kernel_line = newest (System > Packages > Kernel line).\n"
             "# Edit the setting, not this file: Horcrux rewrites it.\n"
             "sys-kernel/gentoo-kernel-bin ~amd64\nsys-kernel/gentoo-kernel ~amd64\nvirtual/dist-kernel ~amd64\n")
GOLDEN_MASK = ("# Written by Horcrux: system.kernel_line = newest (System > Packages > Kernel line).\n"
               "# Edit the setting, not this file: Horcrux rewrites it.\n"
               "# sys-fs/zfs-kmod-2.4.4 builds against kernels up to 6.18: newer kernel lines wait for it.\n"
               ">=sys-kernel/gentoo-kernel-bin-6.19\n>=sys-kernel/gentoo-kernel-6.19\n>=virtual/dist-kernel-6.19\n")

PROFILE = {"kernel_line_choice": True, "kernel_line_state": "/var/lib/horcrux/kernel-line.json", "kernel_line_pin": "horcrux-kernel-line"}


class FakeInst:
    def __init__(self, target):
        self.target, self.logs = target, []

    def log(self, m):
        self.logs.append(m)

    def write(self, rel, text, mode=0o644, append=False):
        p = os.path.join(self.target, rel.lstrip("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
        os.chmod(p, mode)


class KernelLine(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.t)

    def zfs(self, ver, kmax):
        d = os.path.join(self.t, "var/db/pkg/sys-fs", f"zfs-kmod-{ver}")
        os.makedirs(d)
        with open(os.path.join(d, f"zfs-kmod-{ver}.ebuild"), "w") as f:
            f.write(f'EAPI=8\nMODULES_KERNEL_MIN=5.10\nMODULES_KERNEL_MAX={kmax}\n')

    def test_pin_text_matches_the_manager(self):
        self.assertEqual(KL.pins("newest", "amd64", ("6.18", "sys-fs/zfs-kmod-2.4.4")), (GOLDEN_KW, GOLDEN_MASK))
        self.assertEqual(KL.pins("default", "amd64", ("6.18", "x")), (None, None))
        self.assertEqual(KL.pins("newest", "amd64", None), (GOLDEN_KW, None))

    def test_cap_is_the_newest_installed_zfs_module(self):
        self.assertIsNone(KL.zfs_cap(self.t))
        self.zfs("2.3.9", "6.17")
        self.zfs("2.4.4", "6.18")
        self.assertEqual(KL.zfs_cap(self.t), ("6.18", "sys-fs/zfs-kmod-2.4.4"))

    def test_apply_newest_then_default(self):
        self.zfs("2.4.4", "6.18")
        inst = FakeInst(self.t)
        KL.apply(inst, "newest", PROFILE, arch="amd64")
        kw = os.path.join(self.t, "etc/portage/package.accept_keywords/horcrux-kernel-line")
        mask = os.path.join(self.t, "etc/portage/package.mask/horcrux-kernel-line")
        self.assertEqual(open(kw).read(), GOLDEN_KW)
        self.assertEqual(open(mask).read(), GOLDEN_MASK)
        st = json.load(open(os.path.join(self.t, "var/lib/horcrux/kernel-line.json")))
        self.assertEqual(st["line"], "newest")
        KL.apply(inst, "default", PROFILE, arch="amd64")
        self.assertFalse(os.path.exists(kw) or os.path.exists(mask))
        self.assertEqual(json.load(open(os.path.join(self.t, "var/lib/horcrux/kernel-line.json")))["line"], "default")

    def test_answer_file_key_and_validation(self):
        prof = {"name": "horcrux", "title": "Horcrux", "filesystems": ["ext4", "xfs", "zfs"], "swap_mb": 0, "network_mode": "management",
                "services": [], "web_scheme": "https", "web_port": 8006, "default_hostname": "h.localdomain"}

        def cfg(text, profile):
            p = os.path.join(self.t, "a.toml")
            with open(p, "w") as f:
                f.write(text)
            return S.config_from_answers(profile, p)
        self.assertEqual(cfg("[global]\nhostname = \"a.example.com\"\n", dict(prof, **PROFILE))["kernel_line"], "default")
        c = cfg("[global]\nhostname = \"a.example.com\"\n[system]\nkernel_line = \"Newest\"\n", dict(prof, **PROFILE))
        self.assertEqual(c["kernel_line"], "newest")
        self.assertFalse(any("kernel_line" in e for e in S.validate_config(dict(prof, **PROFILE), c)))
        # a product without the page refuses the key; an unknown line is named
        self.assertTrue(any("does not offer" in e for e in S.validate_config(prof, c)))
        c["kernel_line"] = "bleeding"
        self.assertTrue(any("system.kernel_line 'bleeding' unknown" in e for e in S.validate_config(dict(prof, **PROFILE), c)))

    def test_stage_offered_only_with_the_profile_key(self):
        st = ST.BY_ID["kernel"]
        self.assertTrue(st.applies(ST.Ctx(PROFILE, {})))
        self.assertFalse(st.applies(ST.Ctx({}, {})))
        c = {"kernel_line": "default"}
        ctx = ST.Ctx(PROFILE, c)
        f = st.fields(ctx)[0]
        self.assertEqual([v for v, _ in f.options], ["default", "newest"])
        st.commit(ctx, {"line": "newest"})
        self.assertEqual(c["kernel_line"], "newest")
        self.assertIn("newest", st.plan(ctx)[0])


if __name__ == "__main__":
    unittest.main()
