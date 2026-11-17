import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PRUNE = os.path.join(HERE, "..", "vdb-prune.py")


class VdbPrune(unittest.TestCase):
    def mkroot(self, contents, files):
        root = tempfile.mkdtemp()
        d = os.path.join(root, "var/db/pkg/dev-libs/foo-1")
        os.makedirs(d)
        open(os.path.join(d, "CONTENTS"), "w").write(contents)
        for f in files:
            p = os.path.join(root, f.lstrip("/"))
            os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "w").write("x")
        return root, os.path.join(d, "CONTENTS")

    def run_prune(self, root, *extra):
        subprocess.check_call([sys.executable, PRUNE, *extra, root], stdout=subprocess.DEVNULL)

    def test_drops_only_allowlisted_missing_entries(self):
        c = ("dir /usr/include/foo\n"
             "obj /usr/include/foo/a.h d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/share/doc/foo/README d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/lib64/libfoo.a d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/lib64/libfoo.so.1 d41d8cd98f00b204e9800998ecf8427e 1\n"          # not allowlisted, missing: must stay
             "obj /lib/modules/6.1/kernel/drivers/net/can/dev/can-dev.ko d41d8cd98f00b204e9800998ecf8427e 1\n"  # the tar --exclude=dev/* casualty: must stay
             "obj /usr/include/kept.h d41d8cd98f00b204e9800998ecf8427e 1\n")           # present: must stay
        root, cf = self.mkroot(c, ["/usr/include/kept.h"])
        self.run_prune(root)
        left = open(cf).read()
        self.assertIn("libfoo.so.1", left)
        self.assertIn("can-dev.ko", left)
        self.assertIn("kept.h", left)
        self.assertNotIn("a.h", left)
        self.assertNotIn("README", left)
        self.assertNotIn("libfoo.a", left)

    def test_firmware_group_is_opt_in(self):
        c = "obj /lib/firmware/amdgpu/x.bin d41d8cd98f00b204e9800998ecf8427e 1\n"
        root, cf = self.mkroot(c, [])
        self.run_prune(root)
        self.assertIn("amdgpu", open(cf).read())
        self.run_prune(root, "--groups", "core,dev,gringotts-fw")
        self.assertNotIn("amdgpu", open(cf).read())

    def test_desktop_size_group_covers_the_public_edition_strip_only(self):
        """the gnome-light size strip (nvidia GSP firmware, Noto extra variants, thin/black CJK weights) must not leave 'missing file' entries behind,
        while the kept weights and every other firmware directory stay visible to the package-contents gate"""
        h = "d41d8cd98f00b204e9800998ecf8427e 1\n"
        c = (f"obj /usr/lib/firmware/nvidia/ad102/gsp/gsp-535.bin {h}"
             f"obj /usr/share/fonts/noto/NotoSansArabic-SemiBold.ttf {h}"
             f"obj /usr/share/fonts/noto/NotoSans-ExtraCondensedItalic.ttf {h}"
             f"obj /usr/share/fonts/noto/NotoSerifCJK-Light.ttc {h}"
             f"obj /usr/share/fonts/noto-cjk/NotoSansCJK-Thin.ttc {h}"
             f"obj /usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc {h}"
             f"obj /usr/lib/firmware/amdgpu/x.bin {h}")
        root, cf = self.mkroot(c, [])
        self.run_prune(root, "--groups", "core,dev")
        self.assertIn("gsp-535", open(cf).read())          # the group is opt-in
        self.run_prune(root, "--groups", "core,dev,desktop-size")
        left = open(cf).read()
        for gone in ("gsp-535", "Arabic-SemiBold", "ExtraCondensed", "CJK-Thin"):
            self.assertNotIn(gone, left)
        for kept in ("NotoSerifCJK-Light.ttc", "NotoSansCJK-Regular", "amdgpu"):   # Serif CJK Light is under /noto/, not /noto-cjk/: untouched; Regular stays
            self.assertIn(kept, left)


class StripLiveGuiKeepsSharedFiles(unittest.TestCase):
    def test_a_file_also_owned_by_a_staying_package_is_not_deleted(self):
        """gate package-contents-complete (2026-11-04): glib's giomodule.cache was deleted with a live-only package that lists it too"""
        import shutil
        sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
        from liveinst import system
        root = tempfile.mkdtemp()
        try:
            def pkg(name, lines):
                d = os.path.join(root, "var/db/pkg", name)
                os.makedirs(d)
                open(os.path.join(d, "CONTENTS"), "w").write("".join(lines))
            for f in ("usr/lib64/gio/modules/giomodule.cache", "usr/lib64/libgtk.so", "usr/lib64/libglib.so"):
                p = os.path.join(root, f)
                os.makedirs(os.path.dirname(p), exist_ok=True)
                open(p, "w").write("x")
            pkg("dev-libs/glib-2.88", ["obj /usr/lib64/libglib.so d41d8cd98f00b204e9800998ecf8427e 1\n", "obj /usr/lib64/gio/modules/giomodule.cache d41d8cd98f00b204e9800998ecf8427e 1\n"])
            pkg("gui-libs/gtk-4.20", ["obj /usr/lib64/libgtk.so d41d8cd98f00b204e9800998ecf8427e 1\n", "obj /usr/lib64/gio/modules/giomodule.cache d41d8cd98f00b204e9800998ecf8427e 1\n"])
            os.makedirs(os.path.join(root, "etc/live-installer"))
            open(os.path.join(root, "etc/live-installer/live-only-packages"), "w").write("=gui-libs/gtk-4.20\n")

            class Stub:
                target = root
                log = staticmethod(lambda m: None)
            system.Installer.strip_live_gui(Stub())
            self.assertFalse(os.path.exists(os.path.join(root, "usr/lib64/libgtk.so")))
            self.assertFalse(os.path.exists(os.path.join(root, "var/db/pkg/gui-libs/gtk-4.20")))
            self.assertTrue(os.path.exists(os.path.join(root, "usr/lib64/gio/modules/giomodule.cache")), "shared file deleted")
            self.assertTrue(os.path.exists(os.path.join(root, "usr/lib64/libglib.so")))
        finally:
            shutil.rmtree(root)


class StripLiveGuiDropsTheGuiUseFlags(unittest.TestCase):
    def test_server_loses_the_graphical_installer_use_lines(self):
        """freetype[harfbuzz] left on a server pulled harfbuzz[cairo] -> x11-libs/cairo into `emerge @world` (lab hx-installer-vm 2026-11-17)"""
        import shutil
        sys.path.insert(0, os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer"))
        from liveinst import system
        root = tempfile.mkdtemp()
        try:
            os.makedirs(os.path.join(root, "etc/portage/package.use"))
            open(os.path.join(root, "etc/portage/package.use/gui"), "w").write("media-libs/freetype harfbuzz\n")
            open(os.path.join(root, "etc/portage/package.use/horcrux-virt"), "w").write("app-emulation/qemu spice\n")
            logs = []

            class Stub:
                target = root
                log = staticmethod(logs.append)
            system.Installer.strip_live_gui(Stub())          # no live-only-packages list: the USE lines must go anyway
            self.assertFalse(os.path.exists(os.path.join(root, "etc/portage/package.use/gui")))
            self.assertTrue(os.path.exists(os.path.join(root, "etc/portage/package.use/horcrux-virt")))
            self.assertTrue(any("USE flags" in l for l in logs))
            system.Installer.strip_live_gui(Stub())          # idempotent
        finally:
            shutil.rmtree(root)

    def test_desktop_keeps_them(self):
        src = open(os.path.join(HERE, "..", "overlay", "usr", "lib", "live-installer", "liveinst", "system.py")).read()
        self.assertRegex(src, r"if not TYPE_KEEP_GUI\.get\(c\[\"install_type\"\], False\) and not desk:\n\s+self\.strip_live_gui\(\)")


class DistKernelScratchInitrd(unittest.TestCase):
    def test_scratch_initrd_in_the_kernel_tree_is_pruned_only_there(self):
        c = ("obj /usr/src/linux-6.18.60-gentoo-dist-bin/arch/x86/boot/initrd d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /usr/src/linux-6.18.60-gentoo-dist-bin/arch/x86/boot/uki.efi d41d8cd98f00b204e9800998ecf8427e 1\n"
             "obj /boot/initramfs-6.18.60.img d41d8cd98f00b204e9800998ecf8427e 1\n")
        root = tempfile.mkdtemp()
        d = os.path.join(root, "var/db/pkg/sys-kernel/gentoo-kernel-bin-6.18.60")
        os.makedirs(d)
        open(os.path.join(d, "CONTENTS"), "w").write(c)
        subprocess.check_call([sys.executable, PRUNE, root], stdout=subprocess.DEVNULL)
        left = open(os.path.join(d, "CONTENTS")).read()
        self.assertNotIn("arch/x86/boot/initrd", left)
        self.assertNotIn("uki.efi", left)
        self.assertIn("/boot/initramfs-6.18.60.img", left)       # a missing REAL initramfs stays visible

    def test_generated_giomodule_cache_is_pruned(self):
        root = tempfile.mkdtemp()
        d = os.path.join(root, "var/db/pkg/dev-libs/glib-2.88.2")
        os.makedirs(d)
        open(os.path.join(d, "CONTENTS"), "w").write("obj /usr/lib64/gio/modules/giomodule.cache d41d8cd98f00b204e9800998ecf8427e 1\n"
                                                     "obj /usr/lib64/libglib-2.0.so.0 d41d8cd98f00b204e9800998ecf8427e 1\n")
        subprocess.check_call([sys.executable, PRUNE, root], stdout=subprocess.DEVNULL)
        left = open(os.path.join(d, "CONTENTS")).read()
        self.assertNotIn("giomodule.cache", left)
        self.assertIn("libglib-2.0.so.0", left)


if __name__ == "__main__":
    unittest.main()
