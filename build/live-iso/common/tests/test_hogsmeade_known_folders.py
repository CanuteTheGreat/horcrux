"""HOGSMEADE OneDrive known folders (app-misc/hogsmeade-microsoft >= 1-r2, HOGSMEADE-only layer): at login hogsmeade-known-folders links
~/Desktop, ~/Documents, ~/Downloads ... to ~/OneDrive-Personal/<same> (as on his Windows install) and writes user-dirs.dirs. Owner rule
(2026-11-13): it must never move or delete files; a real folder with content is reported and left alone. Runs the real script in a temp
HOME. Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-microsoft"
SCRIPT = PKG / "files" / "hogsmeade-known-folders"
DESKTOP = PKG / "files" / "hogsmeade-known-folders.desktop"
FOLDERS = ["Desktop", "Downloads", "Templates", "Public", "Documents", "Music", "Pictures", "Videos"]


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeKnownFolders(unittest.TestCase):
    def setUp(self):
        if not SCRIPT.is_file():
            self.skipTest("Horcrux repo tree only")
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *args):
        env = {"HOME": str(self.home), "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        return subprocess.run(["bash", str(SCRIPT), *args], env=env, capture_output=True, text=True, timeout=60)

    def snapshot(self):
        out = {}
        for p in self.home.rglob("*"):
            if p.is_file() and not p.is_symlink() and ".local" not in p.parts and ".config" not in p.parts:
                out[str(p.relative_to(self.home))] = p.read_bytes()
        return out

    def test_without_onedrive_nothing_changes(self):
        (self.home / "Downloads").mkdir()
        r = self.run_script()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertTrue((self.home / "Downloads").is_dir() and not (self.home / "Downloads").is_symlink())
        self.assertFalse((self.home / ".config" / "user-dirs.dirs").exists())

    def test_fresh_login_links_every_folder_and_writes_user_dirs(self):
        (self.home / "OneDrive-Personal").mkdir()
        (self.home / "Downloads").mkdir()  # empty, as xdg-user-dirs-update leaves it at first login
        r = self.run_script()
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        for f in FOLDERS:
            link = self.home / f
            self.assertTrue(link.is_symlink(), f)
            self.assertEqual(os.readlink(link), f"OneDrive-Personal/{f}")
            self.assertTrue(link.resolve().is_dir())
        self.assertEqual(os.readlink(self.home / "Screenshots"), "OneDrive-Personal/Pictures/Screenshots")
        dirs = (self.home / ".config" / "user-dirs.dirs").read_text()
        self.assertIn('XDG_DOWNLOAD_DIR="$HOME/OneDrive-Personal/Downloads"', dirs)
        self.assertIn('XDG_PICTURES_DIR="$HOME/OneDrive-Personal/Pictures"', dirs)
        self.assertEqual(len(re.findall(r"(?m)^XDG_\w+_DIR=", dirs)), 8)
        self.assertIn("enabled=False", (self.home / ".config" / "user-dirs.conf").read_text())
        # idempotent: a second login changes nothing and --check passes
        before = (self.home / ".config" / "user-dirs.dirs").stat().st_mtime_ns
        self.assertEqual(self.run_script().returncode, 0)
        self.assertEqual((self.home / ".config" / "user-dirs.dirs").stat().st_mtime_ns, before)
        self.assertEqual(self.run_script("--check").returncode, 0)

    def test_real_folder_with_files_is_never_touched(self):
        od = self.home / "OneDrive-Personal" / "Documents"
        od.mkdir(parents=True)
        (od / "same.txt").write_text("cloud")
        docs = self.home / "Documents"
        (docs / "sub").mkdir(parents=True)
        (docs / "same.txt").write_text("local")
        (docs / "sub" / "x.bin").write_bytes(b"\0" * 10)
        (self.home / ".config").mkdir()
        (self.home / ".config" / "user-dirs.dirs").write_text('XDG_DOCUMENTS_DIR="$HOME/Documents"\n')
        before = self.snapshot()
        r = self.run_script()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("real folder with files: left alone", r.stdout)
        self.assertTrue(docs.is_dir() and not docs.is_symlink())
        self.assertEqual(self.snapshot(), before)  # every file, byte for byte, in its place
        self.assertEqual((self.home / ".config" / "user-dirs.dirs.hogsmeade-bak").read_text(), 'XDG_DOCUMENTS_DIR="$HOME/Documents"\n')
        self.assertIn("left alone", (self.home / ".local" / "state" / "hogsmeade-known-folders.log").read_text())
        self.assertNotEqual(self.run_script("--check").returncode, 0)

    def test_foreign_symlink_is_left_alone(self):
        (self.home / "OneDrive-Personal").mkdir()
        (self.home / "elsewhere").mkdir()
        os.symlink("elsewhere", self.home / "Videos")
        r = self.run_script()
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(os.readlink(self.home / "Videos"), "elsewhere")

    def test_script_has_no_file_removal(self):
        t = SCRIPT.read_text()
        code = "\n".join(l for l in t.splitlines() if not l.lstrip().startswith("#"))
        self.assertNotRegex(code, r"\brm\s")
        self.assertNotRegex(code, r"\bfind\b.*-delete")
        # the only mv renames its own temp file over the config file
        self.assertEqual(re.findall(r"\bmv\b[^\n]*", code), ['mv -f "${f}.tmp.$$" "${f}" && note "wrote ${f}"'])
        self.assertIn("rmdir", code)  # an EMPTY folder only

    def test_packaged_with_autostart_in_hogsmeade_layer_only(self):
        ebs = sorted(PKG.glob("hogsmeade-microsoft-*.ebuild"))
        self.assertEqual(len(ebs), 1)
        eb = ebs[0].read_text()
        self.assertIn('dobin "${FILESDIR}"/hogsmeade-known-folders', eb)
        self.assertIn("insinto /etc/xdg/autostart", eb)
        self.assertIn("Exec=/usr/bin/hogsmeade-known-folders", DESKTOP.read_text())
        self.assertTrue(os.access(SCRIPT, os.X_OK))
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == Path(__file__).name:
                    continue
                if HSM in p.parents:
                    continue
                try:
                    if "hogsmeade-known-folders" in p.read_text():
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "the OneDrive known folders belong to the HOGSMEADE layer only")


if __name__ == "__main__":
    unittest.main()
