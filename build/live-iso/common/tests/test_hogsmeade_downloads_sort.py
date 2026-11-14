"""HOGSMEADE Downloads sorter (app-misc/hogsmeade-downloads-sort, HOGSMEADE-only layer). Owner request 2026-11-13: "I have a mess of
files in Downloads. I need them to get sorted somehow". Rules under test: moves only (never delete/overwrite, " (2)" on a clash),
files/folders touched in the last 14 days and in-progress downloads stay, a top-level folder moves as one unit by its dominant content,
>500 loose files in a bucket are sub-sorted by year, every move is logged and the undo script restores the exact tree; and the
OneDrive guard: destination folders must be reported online before moving, a move the client re-uploads stops the run.
Runs the real script on a temp tree. Reads the Horcrux repo tree; skipped in the vendored copy of common/ in Patronus."""
import json
import os
import re
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = Path(HERE).parents[3]
LIVE = REPO / "build" / "live-iso"
HSM = LIVE / "project" / "variants" / "hogsmeade"
PKG = HSM / "local-repo" / "app-misc" / "hogsmeade-downloads-sort"
SCRIPT = PKG / "files" / "hogsmeade-downloads-sort"
CRON = PKG / "files" / "hogsmeade-downloads-sort.cron"
OLD = time.time() - 400 * 86400


@unittest.skipUnless((REPO / "gentoo").is_dir(), "Horcrux repo tree only")
class HogsmeadeDownloadsSort(unittest.TestCase):
    def setUp(self):
        if not SCRIPT.is_file():
            self.skipTest("Horcrux repo tree only")
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        self.sync = self.home / "OneDrive-Personal"
        self.dl = self.sync / "Downloads"
        self.dl.mkdir(parents=True)
        os.symlink("OneDrive-Personal/Downloads", self.home / "Downloads")
        self.state = self.home / ".local" / "state" / "downloads-sort"

    def tearDown(self):
        self.tmp.cleanup()

    def put(self, rel, data=b"x", mtime=OLD):
        p = self.dl / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        os.utime(p, (mtime, mtime))
        return p

    def run_sort(self, *args, timeout=120):
        env = {"HOME": str(self.home), "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
        return subprocess.run(["python3", str(SCRIPT), *args, "--settle-quiet", "1", "--start-quiet", "1"], env=env, capture_output=True,
                              text=True, timeout=timeout)

    def snapshot(self):
        return {str(p.relative_to(self.dl)): (p.read_bytes(), p.stat().st_mtime) for p in self.dl.rglob("*") if p.is_file()}

    def test_sort_log_and_undo(self):
        self.put("report.pdf", b"pdf")
        self.put("photo.JPG", b"jpg")
        self.put("app-icon-512.png.1", b"png")
        self.put("clip.mp4", b"mp4" * 100)
        self.put("backup.tar.gz", b"tgz")
        self.put("setup.exe", b"MZ")
        self.put("tool.deb", b"deb")
        self.put("game.apk", b"apk")
        self.put("disk.iso", b"iso")
        self.put("font.ttf", b"ttf")
        self.put("part.stl", b"stl")
        self.put("key.pem", b"pem")
        self.put("book.epub", b"epub")
        self.put("unknown.xyz", b"?")
        self.put("noext", b"?")
        self.put("Images/photo.JPG", b"older photo already there")  # clash -> " (2)"
        self.put("recent.pdf", b"new", mtime=time.time() - 3 * 86400)  # last 14 days: stays
        self.put("big.iso.part", b"dl")  # in progress: stays
        self.put("x.crdownload", b"dl")
        self.put(".hidden", b"h")
        for i in range(5):
            self.put(f"holiday/IMG_{i}.jpg", b"j" * (i + 1))
        self.put("holiday/notes.txt", b"n")
        self.put("myrepo/.git/HEAD", b"ref")
        self.put("myrepo/main.rs", b"fn main(){}")
        self.put("hashcat-6.2.6/hashcat.exe", b"MZ")
        self.put("hashcat-6.2.6/kern.cl", b"k")
        self.put("hashcat-6.2.6/kern2.cl", b"k")
        self.put("mixed/a.pdf", b"a")
        self.put("mixed/b.mp4", b"b")
        self.put("mixed/c.wav", b"c")
        self.put("fresh-folder/old.pdf", b"o")
        self.put("fresh-folder/new.pdf", b"n", mtime=time.time() - 86400)  # one recent file keeps the whole folder in place
        before = self.snapshot()

        self.assertEqual(self.run_sort("inventory").returncode, 0)
        inv = json.loads((self.state / "inventory.json").read_text())
        self.assertEqual(inv["total_files"], len(before))
        r = self.run_sort("apply")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        want = {
            "Documents/report.pdf", "Images/photo.JPG", "Images/photo (2).JPG", "Images/app-icon-512.png.1", "Videos/clip.mp4",
            "Archives/backup.tar.gz", "Installers/Windows/setup.exe", "Installers/Linux/tool.deb", "Installers/Android/game.apk",
            "Disk Images/disk.iso", "Fonts/font.ttf", "3D-Print/part.stl", "Keys/key.pem", "Documents/Books/book.epub",
            "Other/unknown.xyz", "Other/noext", "recent.pdf", "big.iso.part", "x.crdownload", ".hidden",
            "Images/holiday/IMG_0.jpg", "Images/holiday/notes.txt", "Code/myrepo/main.rs", "Code/myrepo/.git/HEAD",
            "Software/hashcat-6.2.6/hashcat.exe", "Folders/mixed/a.pdf", "fresh-folder/old.pdf", "fresh-folder/new.pdf",
        }
        after = self.snapshot()
        self.assertTrue(want <= set(after), sorted(want - set(after)))
        self.assertEqual(after["Images/photo.JPG"][0], b"older photo already there")  # never overwritten
        self.assertEqual(after["Images/photo (2).JPG"][0], b"jpg")
        # nothing lost: same bytes, same mtimes, only paths changed
        self.assertEqual(sorted(after.values()), sorted(before.values()))
        v = self.run_sort("verify")
        self.assertEqual(v.returncode, 0, v.stdout + v.stderr)
        day = time.strftime("%Y%m%d")
        rows = (self.state / f"moves-{day}.tsv").read_text().splitlines()
        self.assertEqual(rows[0], "old\tnew\tsize\tmtime")
        self.assertTrue(any(r.startswith(str(self.dl / "photo.JPG") + "\t" + str(self.dl / "Images" / "photo (2).JPG") + "\t3\t") for r in rows))
        # a second run has nothing to do
        self.run_sort("apply")
        self.assertEqual(self.snapshot(), after)
        # undo puts every file back where it was
        u = subprocess.run(["bash", str(self.state / f"undo-{day}.sh")], capture_output=True, text=True)
        self.assertEqual(u.returncode, 0, u.stderr)
        self.assertEqual(self.snapshot(), before)

    def test_year_subfolders_over_500(self):
        for i in range(501):
            self.put(f"scan{i:04d}.pdf", b"p", mtime=OLD if i % 2 else OLD - 400 * 86400)
        self.put("one.png", b"i")
        r = self.run_sort("apply")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        years = {p.name for p in (self.dl / "Documents").iterdir()}
        self.assertTrue(all(re.fullmatch(r"20\d\d", y) for y in years) and len(years) == 2, years)
        self.assertEqual(sum(1 for _ in (self.dl / "Documents").rglob("*.pdf")), 501)
        self.assertTrue((self.dl / "Images" / "one.png").is_file())  # small bucket stays flat

    def test_onedrive_guard(self):
        """Folders first, moves only after the client reports them online; a re-upload of moved data stops the run."""
        log = self.home / "od.log"
        log.write_text("Sync with Microsoft OneDrive is complete\n")
        status = self.home / "monitor-status.json"
        fake = self.home / "onedrive"  # a process whose command line says onedrive, as the real client's does
        os.symlink("/bin/sleep", fake)
        proc = subprocess.Popen([str(fake), "300"])
        self.addCleanup(proc.wait)
        self.addCleanup(proc.kill)
        status.write_text(json.dumps({"pid": proc.pid, "process": {"state": "idle"}}))
        for i in range(3):
            self.put(f"vids/{i}.mp4", b"v" * 10)
        self.put("doc.pdf", b"d")
        stop = threading.Event()
        seen_early = []

        def client():  # fake onedrive: creates new folders online, then (badly) re-uploads the moved video folder
            done = set()
            while not stop.is_set():
                for d in ("Videos", "Documents"):
                    p = self.dl / d
                    if p.is_dir() and d not in done:
                        if any(p.iterdir()):
                            seen_early.append(d)  # something moved in before the folder was online
                        with open(log, "a") as fh:
                            fh.write(f"Successfully created the remote directory ./Downloads/{d} on Microsoft OneDrive\n")
                        done.add(d)
                if (self.dl / "Videos" / "vids").is_dir() and "up" not in done:
                    with open(log, "a") as fh:
                        fh.write("Successfully created the remote directory ./Downloads/Videos/vids on Microsoft OneDrive\n")
                        fh.write("Uploading new file: ./Downloads/Videos/vids/0.mp4 ... done\n")
                    done.add("up")
                time.sleep(0.2)

        t = threading.Thread(target=client)
        t.start()
        try:
            r = self.run_sort("apply", "--onedrive-log", str(log), "--onedrive-status", str(status), "--max-wait", "30")
        finally:
            stop.set()
            t.join()
        self.assertEqual(r.returncode, 4, r.stdout + r.stderr)  # ABORT
        self.assertEqual(seen_early, [])
        self.assertTrue((self.dl / "Videos" / "vids" / "0.mp4").is_file())
        self.assertTrue((self.dl / "doc.pdf").is_file())  # nothing more moved after the abort
        self.assertIn("ABORT", (self.state / "progress.log").read_text())

    def test_stopped_client_means_no_moves(self):
        status = self.home / "monitor-status.json"
        status.write_text(json.dumps({"pid": 0, "process": {"state": "idle"}}))
        log = self.home / "od.log"
        log.write_text("")
        self.put("doc.pdf", b"d")
        r = self.run_sort("apply", "--onedrive-log", str(log), "--onedrive-status", str(status))
        self.assertEqual(r.returncode, 3, r.stdout + r.stderr)
        self.assertTrue((self.dl / "doc.pdf").is_file())

    def test_online_leftover_defers(self):
        """An interrupted earlier run left ./Downloads/Documents/old online (created, never deleted): moving a local 'old'
        folder onto that path would collide online, so it waits; once the client deletes the leftover, it moves."""
        fake = self.home / "onedrive"
        os.symlink("/bin/sleep", fake)
        proc = subprocess.Popen([str(fake), "300"])
        self.addCleanup(proc.wait)
        self.addCleanup(proc.kill)
        status = self.home / "monitor-status.json"
        status.write_text(json.dumps({"pid": proc.pid, "process": {"state": "idle"}}))
        (self.dl / "Documents").mkdir()
        log = self.home / "od.log"
        log.write_text("ERROR: an old error that is not this run's\n"
                       "Successfully created the remote directory ./Downloads/Documents/old on Microsoft OneDrive\n"
                       "Successfully created the remote directory ./Downloads/Documents/gone on Microsoft OneDrive\n"
                       "Deleting item from Microsoft OneDrive: ./Downloads/Documents/gone\n")
        self.put("old/a.pdf", b"a")
        self.put("gone/b.pdf", b"b")
        args = ("apply", "--onedrive-log", str(log), "--onedrive-status", str(status))
        r = self.run_sort(*args)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("deferred 1", r.stdout)
        self.assertIn("onedrive error lines 0", r.stdout)
        self.assertTrue((self.dl / "old" / "a.pdf").is_file())
        self.assertTrue((self.dl / "Documents" / "gone" / "b.pdf").is_file())
        with open(log, "a") as fh:
            fh.write("Deleting item from Microsoft OneDrive: ./Downloads/Documents/old\n")
        r = self.run_sort(*args)
        self.assertIn("deferred 0", r.stdout)
        self.assertTrue((self.dl / "Documents" / "old" / "a.pdf").is_file())

    def test_script_never_deletes(self):
        code = "\n".join(l for l in SCRIPT.read_text().splitlines() if not l.lstrip().startswith("#"))
        self.assertNotRegex(code, r"shutil|rmtree|os\.unlink|\.unlink\(|os\.rmdir|\brm -")
        # the one os.remove is the sorter's own bookkeeping file, and os.replace only renames its own temp files
        self.assertEqual(re.findall(r"os\.remove\([^)]*\)", code), ["os.remove(pending_f)"])
        self.assertEqual(sorted(set(re.findall(r"os\.replace\(([^,]+),", code))), ['out + ".tmp"', 'undo + ".tmp"'])
        self.assertIn("RENAME_NOREPLACE", code)
        self.assertIn("mv -n", code)

    def test_packaged_cron_in_hogsmeade_layer_only(self):
        ebs = sorted(PKG.glob("hogsmeade-downloads-sort-*.ebuild"))
        self.assertEqual(len(ebs), 1)
        eb = ebs[0].read_text()
        self.assertIn('python_newscript "${FILESDIR}"/hogsmeade-downloads-sort', eb)
        self.assertIn("insinto /etc/cron.d", eb)
        self.assertIn("virtual/cron", eb)
        self.assertTrue(os.access(SCRIPT, os.X_OK))
        line = [l for l in CRON.read_text().splitlines() if l and not l.startswith("#") and "=" not in l.split()[0]]
        self.assertEqual(len(line), 1)
        f = line[0].split()
        self.assertEqual(f[5], "canutethegreat")  # runs as the owner, so ownership/permissions stay his
        self.assertIn("ionice -c 3", line[0])
        self.assertIn("hogsmeade-downloads-sort nightly", line[0])
        self.assertIn("app-misc/hogsmeade-downloads-sort", (HSM / "Dockerfile.part2").read_text())
        bad = []
        for r in (REPO / "gentoo", LIVE / "common", LIVE / "project"):
            for p in r.rglob("*"):
                if not p.is_file() or "__pycache__" in p.parts or p.suffix == ".pyc" or p.name == Path(__file__).name:
                    continue
                if HSM in p.parents:
                    continue
                try:
                    if "hogsmeade-downloads-sort" in p.read_text():
                        bad.append(str(p.relative_to(REPO)))
                except (UnicodeDecodeError, OSError):
                    pass
        self.assertEqual(bad, [], "the Downloads sorter belongs to the HOGSMEADE layer only")


if __name__ == "__main__":
    unittest.main()
