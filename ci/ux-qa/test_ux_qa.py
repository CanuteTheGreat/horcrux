#!/usr/bin/env python3
"""Self-tests for the UX-QA checkers: each one must catch the exact defect it was written for (and pass a clean input).

Run: python3 -m unittest discover -s ci/ux-qa -p 'test_*.py'     (pure python, no browser)
"""
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check_os_release as osr  # noqa: E402
import check_public_text as pub  # noqa: E402
import check_owner_findings as of  # noqa: E402
import ux_crawl as uc  # noqa: E402

GOOD = '''NAME="Horcrux"
ID=horcrux
ID_LIKE=gentoo
PRETTY_NAME="Horcrux 0.1 (Server)"
VERSION_ID="0.1.0"
BUILD_ID="202611122159"
VARIANT="Server"
VARIANT_ID=server
HOME_URL="https://horcrux.errant.solutions/"
DOCUMENTATION_URL="https://horcrux.errant.solutions/"
SUPPORT_URL="https://horcrux.errant.solutions/"
BUG_REPORT_URL="https://github.com/CanuteTheGreat/horcrux/issues"
'''
# what the owner actually found on an installed system, 2026-11-12
OWNER_BAD = '''NAME="Horcrux"
ID=horcrux
PRETTY_NAME="Horcrux live installer"
VARIANT="hogshead GNOME Light"
VARIANT_ID=gnome-light
HOME_URL="https://git.canutethegreat.com/CanuteTheGreat/horcrux"
'''


class OsRelease(unittest.TestCase):
    def test_clean_passes(self):
        self.assertEqual(osr.check(GOOD), [])

    def test_owner_findings_all_caught(self):
        probs = " | ".join(osr.check(OWNER_BAD, hostnames=["hogshead"]))
        for needle in ("live/installer", "internal build name", "machine/host name 'hogshead'", "private", "VERSION_ID"):
            self.assertIn(needle, probs)

    def test_live_medium_may_say_live(self):
        live = GOOD.replace('PRETTY_NAME="Horcrux 0.1 (Server)"', 'PRETTY_NAME="Horcrux 0.1 (Server live)"')
        self.assertTrue(any("live" in p for p in osr.check(live)))
        self.assertEqual(osr.check(live, live=True), [])

    def test_rootfs_dir(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/etc")
            open(f"{d}/etc/os-release", "w").write(OWNER_BAD)
            self.assertNotEqual(os.system(f"{sys.executable} {HERE}/check_os_release.py {d} >/dev/null"), 0)


class PublicText(unittest.TestCase):
    def test_catches_private_url_in_text_and_binary(self):
        with tempfile.TemporaryDirectory() as d:
            open(f"{d}/a.md", "w").write("clone https://git." + "canutethegreat.com/x/y")
            open(f"{d}/b.bin", "wb").write(b"\x00\x01ssh://git@git." + b"canutethegreat.com/x\x00")
            open(f"{d}/ok.md", "w").write("see https://github.com/CanuteTheGreat/horcrux")
            hits = pub.scan_paths([d], pub.default_rules(), allow=[], baseline={})
            files = {h["file"].split("/")[-1] for h in hits if h["sev"] == "fail"}
            self.assertEqual(files, {"a.md", "b.bin"})

    def test_machine_names_warn_in_docs_fail_in_ui(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/docs")
            os.makedirs(f"{d}/ui")
            open(f"{d}/docs/x.md", "w").write("tested on gringotts")
            open(f"{d}/ui/x.html", "w").write("<p>gringotts</p>")
            hits = pub.scan_paths([f"{d}/docs", f"{d}/ui"], pub.default_rules(), allow=[], baseline={}, strict_prefixes=[f"{d}/ui"])
            sev = {h["file"].split("/")[-1]: h["sev"] for h in hits}
            self.assertEqual(sev, {"x.md": "warn", "x.html": "fail"})

    def test_tarball(self):
        import io
        import tarfile
        with tempfile.TemporaryDirectory() as d:
            with tarfile.open(f"{d}/p.tar.gz", "w:gz") as t:
                data = ("home=https://git." + "canutethegreat.com/").encode()
                ti = tarfile.TarInfo("etc/x.conf")
                ti.size = len(data)
                t.addfile(ti, io.BytesIO(data))
            hits = pub.scan_paths([f"{d}/p.tar.gz"], pub.default_rules(), allow=[], baseline={})
            self.assertTrue(any(h["sev"] == "fail" and "etc/x.conf" in h["file"] for h in hits))


class CrawlRules(unittest.TestCase):
    def test_zero_in_red_is_caught_but_not_green_or_normal(self):
        self.assertTrue(uc.is_red("rgb(198, 45, 18)"))
        self.assertTrue(uc.is_red("rgb(255, 107, 107)"))
        self.assertFalse(uc.is_red("rgb(45, 122, 32)"))
        self.assertFalse(uc.is_red("rgb(74, 85, 180)"))
        self.assertFalse(uc.is_red("rgb(255, 255, 255)"))
        for z in ("0", " 0 ", "0%", "0 B", "None"):
            self.assertTrue(uc.is_zero(z), z)
        for n in ("1", "10", "36.5%", "3"):
            self.assertFalse(uc.is_zero(n), n)

    def test_duplicate_titles_need_three_distinct_routes(self):
        pg = lambda r, t: {"route": r, "title": t}  # noqa: E731
        same = [pg("/a", "X"), pg("/b?x=1", "X"), pg("/b", "X"), pg("/c", "Y"), pg("/d", "Y")]
        self.assertEqual(uc.duplicate_titles(same), {})  # /b twice counts once -> X on 2 routes
        same.append(pg("/e", "X"))
        self.assertEqual(list(uc.duplicate_titles(same)), ["X"])


class OwnerFindings(unittest.TestCase):
    def test_each_finding_needs_an_existing_check(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/ci")
            open(f"{d}/ci/real.py", "w").write("")
            md = ("| # | date | finding | class | check |\n|---|---|---|---|---|\n"
                  "| 1 | 2026-11-12 | a | x | `ci/real.py` |\n"
                  "| 2 | 2026-11-12 | b | x | `ci/gone.py` |\n"
                  "| 3 | 2026-11-12 | c | x | (none yet) |\n")
            open(f"{d}/f.md", "w").write(md)
            probs = of.check(f"{d}/f.md", d)
            self.assertEqual(len(probs), 2)
            self.assertTrue(any("gone.py" in p for p in probs))
            self.assertTrue(any("no check" in p for p in probs))

    def test_journey_check_must_exist_in_the_journey_script(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/ci/ux-qa")
            open(f"{d}/ci/ux-qa/user_journey.py", "w").write('expect("j3-create-vm", "resource-sliders", x, "y")\n')
            md = ("| # | date | finding | class | check |\n|---|---|---|---|---|\n"
                  "| 1 | 2026-11-16 | a | x | `journey:j3-create-vm:resource-sliders` |\n"
                  "| 2 | 2026-11-16 | b | x | `journey:j3-create-vm:made-up` |\n")
            open(f"{d}/f.md", "w").write(md)
            probs = of.check(f"{d}/f.md", d)
            self.assertEqual(len(probs), 1)
            self.assertIn("made-up", probs[0])


class UnstyledControls(unittest.TestCase):
    """The bare-control / bare-table / jammed-label / placeholder-only rules, run in a real browser on a fixture page."""

    CSS = os.path.join(HERE, "..", "..", "horcrux-api", "horcrux-ui", "style", "main.css")
    BAD = ('<main class="container"><label>Status Filter:</label><select><option>All</option></select>'
           '<input type="text" placeholder="DNS servers for clients (default: this server)">'
           '<button>Go</button><table><tr><th>A</th></tr><tr><td>1</td></tr></table></main>')
    GOOD = ('<main class="container"><div class="filter-controls"><label for="f">Status filter</label><select id="f"><option>All</option></select></div>'
            '<div class="form-group"><label for="d">DNS servers</label><input id="d" type="text" placeholder="1.1.1.1"></div>'
            '<button class="btn btn-primary">Go</button><table class="table"><tr><th>A</th></tr><tr><td>1</td></tr></table></main>')

    def facts(self, body, bare_css):
        try:
            from playwright.sync_api import sync_playwright
        except ImportError:
            self.skipTest("playwright not installed")
        css = "" if bare_css else open(self.CSS).read()
        with sync_playwright() as p:
            try:
                b = p.firefox.launch()
            except Exception as e:  # noqa: BLE001
                self.skipTest(f"no firefox: {e}")
            pg = b.new_page(viewport={"width": 1280, "height": 800})
            pg.set_content(f"<style>{css}</style>{body}")
            f = pg.evaluate(uc.FACTS_JS)
            b.close()
        return f

    def test_browser_default_controls_are_caught(self):
        f = self.facts(self.BAD, bare_css=True)
        self.assertTrue(f["bareControls"], f)
        self.assertTrue(f["bareTables"], f)
        self.assertTrue(f["jammedLabels"], f)
        self.assertTrue(f["placeholderOnly"], f)

    def test_shared_classes_pass(self):
        f = self.facts(self.GOOD, bare_css=False)
        for k in ("bareControls", "bareTables", "jammedLabels", "placeholderOnly", "truncPlaceholder"):
            self.assertEqual(f[k], [], (k, f[k]))

    def test_the_shared_stylesheet_also_rescues_bare_markup(self):
        """main.css styles unclassed controls and tables itself, so a page that forgets the classes still renders properly."""
        f = self.facts(self.BAD, bare_css=False)
        self.assertEqual(f["bareControls"], [], f)
        self.assertEqual(f["bareTables"], [], f)


class AsciiIconTests(unittest.TestCase):
    def test_placeholders_are_caught(self):
        for t in ("[X]No IOMMU groups", "[OK] Yes", "a [!] b", "[D]", "[warn] x"):
            self.assertTrue(uc.ASCII_ICON.search(t), t)

    def test_clean_text_passes(self):
        for t in ("OK", "Disk [1]", "array[0]", "No IOMMU groups", "[Optional]"):
            self.assertFalse(uc.ASCII_ICON.search(t), t)


if __name__ == "__main__":
    unittest.main()
