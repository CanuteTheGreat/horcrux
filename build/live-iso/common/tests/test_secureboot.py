"""UEFI Secure Boot: the build-time signing helpers and the installer's Secure Boot path (pure parts, no firmware needed).

Chain: firmware (Microsoft UEFI CA) -> shim (sys-boot/shim) -> our GRUB -> our kernel -> modules; our certificate is enrolled once
in MokManager. Build side: boot/secureboot-sign.sh, boot/sign-modules.py, boot/resign-initramfs.py. Installer: liveinst/secureboot.py.
QEMU proof: secureboot-test.py (no key refused, MokManager enrollment, SB off) and qemu-test.py --secure-boot mok (installed system).
Run: python3 -m unittest discover -s build/live-iso/common/tests
"""
import importlib.util
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
COMMON = os.path.join(HERE, "..")
LI = os.path.join(COMMON, "overlay", "usr", "lib", "live-installer")
sys.path.insert(0, LI)
from liveinst import secureboot as SB  # noqa: E402
from liveinst import system  # noqa: E402


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


SM = load("sign_modules", os.path.join(COMMON, "boot", "sign-modules.py"))
RI = load("resign_initramfs", os.path.join(COMMON, "boot", "resign-initramfs.py"))
HAVE_OPENSSL = shutil.which("openssl") is not None


def mkcert(d):
    key, crt = os.path.join(d, "k.pem"), os.path.join(d, "c.pem")
    subprocess.run(["openssl", "req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=unit test",
                    "-keyout", key, "-out", crt], check=True, capture_output=True)
    return key, crt


class Installer(unittest.TestCase):
    def test_esp_grub_cfg_finds_boot_by_uuid(self):
        cfg = SB.esp_grub_cfg("1234-ABCD")
        self.assertIn("search --no-floppy --fs-uuid --set=root 1234-ABCD", cfg)
        self.assertIn("configfile ($root)/grub/grub.cfg", cfg)

    def test_mode_validation(self):
        with mock.patch.object(SB, "material", return_value=None):
            self.assertIn("no signed boot chain", SB.mode_problem({"secure_boot": "on"}, True))
            self.assertEqual(SB.mode_problem({"secure_boot": "auto"}, True), "")      # auto on an unsigned image: plain GRUB
            self.assertIn("UEFI", SB.mode_problem({"secure_boot": "on"}, False))
        self.assertIn("auto, on or off", SB.mode_problem({"secure_boot": "maybe"}, True))
        self.assertIn("both key and cert", SB.mode_problem({"secure_boot_key": "/k"}, True))

    def test_active_only_on_uefi_with_material(self):
        inst = mock.Mock(efi=True, c={"secure_boot": "auto"})
        with mock.patch.object(SB, "material", return_value={"suffix": "x64"}):
            self.assertTrue(SB.active(inst))
            inst.c["secure_boot"] = "off"
            self.assertFalse(SB.active(inst))
            inst.c["secure_boot"], inst.efi = "auto", False
            self.assertFalse(SB.active(inst))
        with mock.patch.object(SB, "material", return_value=None):
            inst.efi = True
            self.assertFalse(SB.active(inst))

    def test_portage_config_signs_kernels_and_modules(self):
        use, mk = SB.portage_config("/etc/horcrux/secureboot")
        self.assertIn("sys-kernel/gentoo-kernel-bin secureboot", use)
        self.assertIn("*/* modules-sign", use)
        for v in ("SECUREBOOT_SIGN_KEY", "SECUREBOOT_SIGN_CERT", "MODULES_SIGN_KEY", "MODULES_SIGN_CERT"):
            self.assertIn(f'{v}="/etc/horcrux/secureboot/machine.', mk)

    def test_password_is_typeable(self):
        for _ in range(50):
            pw = SB.gen_password()
            self.assertEqual(len(pw), 8)
            self.assertTrue(set(pw) <= set(SB.PW_ALPHABET))
            self.assertFalse(set(pw) & set("0o1liI"))

    def test_material_needs_every_file(self):
        with tempfile.TemporaryDirectory() as r:
            d = os.path.join(r, SB.SB_DIR.lstrip("/"))
            os.makedirs(d)
            with mock.patch.object(SB, "suffix", return_value="x64"):
                self.assertIsNone(SB.material(r))
                for f in ("shimx64.efi", "mmx64.efi", "grubx64.efi", "ENROLL_THIS_KEY_IN_MOKMANAGER.cer", "signing.crt"):
                    open(os.path.join(d, f), "w").close()
                self.assertEqual(SB.material(r)["suffix"], "x64")
            with mock.patch.object(SB, "suffix", return_value=""):      # riscv64: no signed shim exists
                self.assertIsNone(SB.material(r))

    def test_answer_file_section(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "answers.toml")
            with open(p, "w") as f:
                f.write('[secure_boot]\nmode = "on"\nkey = "mok/machine.key"\ncert = "/abs/machine.crt"\nmok_password = "pw"\nmachine_key = true\n')
            prof = {"name": "horcrux", "title": "Horcrux", "filesystems": ["zfs", "xfs"], "default_hostname": "h.localdomain"}
            c = system.config_from_answers(prof, p)
        self.assertEqual(c["secure_boot"], "on")
        self.assertEqual(c["secure_boot_key"], os.path.join(d, "mok/machine.key"))
        self.assertEqual(c["secure_boot_cert"], "/abs/machine.crt")
        self.assertEqual(c["secure_boot_mok_password"], "pw")

    def test_done_page_rows(self):
        self.assertEqual(SB.done_lines({}), [])
        rows = dict(SB.done_lines({"secure_boot_used": True, "secure_boot_state": "on", "secure_boot_mok_password": "abcd2345"}))
        self.assertEqual(rows["MOK password"], "abcd2345")


@unittest.skipUnless(HAVE_OPENSSL, "openssl missing")
class ModuleSigning(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.key, self.crt = mkcert(self.d)
        self.a = mock.Mock(key=self.key, cert=self.crt, hash="sha256", force=False)

    def tearDown(self):
        shutil.rmtree(self.d)

    def test_appends_kernel_signature_format_once(self):
        ko = os.path.join(self.d, "x.ko")
        with open(ko, "wb") as f:
            f.write(b"\x7fELF fake module body")
        self.assertEqual(SM.handle(ko, self.a), "signed")
        data = open(ko, "rb").read()
        self.assertTrue(data.endswith(SM.MAGIC))
        info = data[-len(SM.MAGIC) - 12:-len(SM.MAGIC)]
        algo, hsh, id_type, signer_len, keyid_len, sig_len = struct.unpack(">BBBBB3xI", info)
        self.assertEqual((id_type, signer_len, keyid_len), (SM.PKEY_ID_PKCS7, 0, 0))
        self.assertEqual(SM.strip_sig(data), b"\x7fELF fake module body")
        # the detached CMS signature verifies over the module body with the certificate
        sig = data[-len(SM.MAGIC) - 12 - sig_len:-len(SM.MAGIC) - 12]
        for n, b in (("sig.der", sig), ("body", SM.strip_sig(data))):
            with open(os.path.join(self.d, n), "wb") as f:
                f.write(b)
        r = subprocess.run(["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-in", os.path.join(self.d, "sig.der"),
                            "-content", os.path.join(self.d, "body"), "-certfile", self.crt, "-nointern", "-noverify", "-out", "/dev/null"],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(SM.handle(ko, self.a), "kept")              # already signed: left alone

    @unittest.skipUnless(shutil.which("cpio") and shutil.which("gzip"), "cpio/gzip missing")
    def test_initramfs_keeps_early_cpio_and_signs_modules(self):
        def cpio_of(root):
            names = subprocess.run("find . -mindepth 1 | LC_ALL=C sort", shell=True, cwd=root, capture_output=True, check=True).stdout
            return subprocess.run(["cpio", "-o", "-H", "newc", "--quiet"], input=names, cwd=root, capture_output=True, check=True).stdout
        early_root, main_root = os.path.join(self.d, "e"), os.path.join(self.d, "m")
        os.makedirs(os.path.join(early_root, "kernel/x86/microcode"))
        open(os.path.join(early_root, "kernel/x86/microcode/GenuineIntel.bin"), "wb").write(b"ucode")
        os.makedirs(os.path.join(main_root, "lib/modules/6.1/extra"))
        open(os.path.join(main_root, "lib/modules/6.1/extra/zfs.ko"), "wb").write(b"zfs module")
        early = cpio_of(early_root)
        early += b"\0" * (512 - len(early) % 512)
        img = os.path.join(self.d, "initramfs.img")
        with open(img, "wb") as f:
            f.write(early + subprocess.run(["gzip", "-c"], input=cpio_of(main_root), capture_output=True, check=True).stdout)
        self.assertEqual(RI.early_len(open(img, "rb").read()), len(early))
        self.assertEqual(RI.run(img, self.a), 0)
        data = open(img, "rb").read()
        self.assertEqual(data[:len(early)], early)                  # microcode archive untouched, still first
        out = os.path.join(self.d, "x")
        os.makedirs(out)
        cp = subprocess.run(["gzip", "-dc"], input=data[len(early):], capture_output=True, check=True).stdout
        subprocess.run(["cpio", "-idm", "--quiet"], input=cp, cwd=out, check=True)
        self.assertTrue(open(os.path.join(out, "lib/modules/6.1/extra/zfs.ko"), "rb").read().endswith(SM.MAGIC))
        before = open(img, "rb").read()
        RI.run(img, self.a)
        self.assertEqual(open(img, "rb").read(), before)            # nothing left to sign: byte-identical


class BuildWiring(unittest.TestCase):
    def test_assemble_signs_before_squashfs_and_uses_shim(self):
        s = open(os.path.join(COMMON, "assemble-iso.sh")).read()
        self.assertLess(s.index("sb_sign_rootfs"), s.index("mksquashfs"))
        self.assertLess(s.index("sb_sign_rootfs"), s.index('cp "$ROOT/boot/vmlinuz-$kver"'))
        self.assertIn("sb_iso_esp", s)

    def test_project_certificate_shipped_and_no_private_key_in_tree(self):
        crt = os.path.join(COMMON, "boot", "secureboot", "signing.crt")
        self.assertIn("BEGIN CERTIFICATE", open(crt).read())
        for d, _, fs in os.walk(COMMON):
            for f in fs:
                p = os.path.join(d, f)
                if f.endswith((".pem", ".key", ".crt")) and os.path.getsize(p) < 100000:
                    self.assertNotIn("PRIVATE KEY-----", open(p, errors="replace").read(), p)

    def test_grub_modules_cover_installed_configs(self):
        s = open(os.path.join(COMMON, "boot", "secureboot-sign.sh")).read()
        mods = s.split('SB_GRUB_MODS="')[1].split('"')[0].split()
        # lockdown: no module loads from disk, so whatever horcrux-be / grub-mkconfig / the live menu insmod must be embedded
        for m in ("loadenv", "search_fs_uuid", "ext2", "xfs", "part_gpt", "linux", "gzio", "all_video", "configfile", "serial", "reboot", "regexp"):
            self.assertIn(m, mods)

    def test_live_menu_skips_fonts_in_lockdown(self):
        out = subprocess.run(["bash", os.path.join(COMMON, "boot", "gen-grub-cfg.sh"), "X_LIVE", "X", "x", "x86_64", "none", "cyan"],
                             env={**os.environ, "GFX_OK": "1", "GRUB_SERIAL": "1", "SERIAL_DEV": "ttyS0"}, capture_output=True, text=True).stdout
        self.assertIn('if [ "$lockdown" != y ] && loadfont', out)

    def test_ci_runs_secure_boot_gates(self):
        s = open(os.path.join(COMMON, "ci-run.sh")).read()
        self.assertIn("secureboot-test.py", s)
        self.assertIn("--secure-boot mok", s)
        self.assertIn("export SECUREBOOT=on", s)

    def test_live_iso_workflows_pass_the_key(self):
        wf = os.path.join(COMMON, "..", "..", "..", ".forgejo", "workflows")
        if not os.path.isdir(wf):
            self.skipTest("workflows not in this checkout")
        for f in os.listdir(wf):
            s = open(os.path.join(wf, f)).read()
            if "ci-run.sh" in s and "RELEASE_SIGNING_KEY: ${{" in s:
                self.assertIn("SECUREBOOT_SIGNING_KEY: ${{ secrets.SECUREBOOT_SIGNING_KEY }}", s, f)


if __name__ == "__main__":
    unittest.main()
