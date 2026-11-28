"""Safety gate (OpenZFS 2.3 -> 2.4 move): nothing in the product ever upgrades a pool on its own.

`zpool upgrade` (or `zpool set feature@x=enabled`) turns on new on-disk features: the pool can then no longer be imported by an older OpenZFS
(an older Horcrux/Patronus ISO used as a rescue system, the Proxmox host it came from, GRUB). Existing pools must keep their feature set across
package updates, so the installer, first boot, horcrux-doctor, horcrux-autoupdate, the API and every packaged script must never run either
command. A pool upgrade is only ever done by an administrator by hand. This test scans the whole repository (code, ebuilds, init scripts,
overlay files) for such a call; comments, docs and tests are not code and are skipped.
"""
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
# common/ lives at <repo>/build/live-iso/common (Horcrux) or <repo>/gentoo/live-iso/common (Patronus): the repo root is three levels above common/
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))

SKIP_DIRS = {".git", "target", "node_modules", "docs", "tests", "test", "fixtures", "__pycache__", "dist", "vendor"}
SKIP_EXT = {".md", ".txt", ".html", ".svg", ".png", ".jpg", ".xz", ".gz", ".zst", ".iso", ".img", ".qcow2", ".lock", ".json", ".asc", ".crt", ".pem"}
# a call in shell (`zpool upgrade`), python (["zpool", "upgrade"]), Rust (Command::new("zpool").arg("upgrade") / .args(["upgrade", ...]))
UPGRADE = re.compile(r"""zpool['"]?\s*[,)]?\s*(?:\.args?\(\s*\[?\s*)?['"]?\s*upgrade\b""")
FEATURE_SET = re.compile(r"""zpool['"]?\s*,?\s*['"]?\s*set\b.*feature@[a-z_]+\s*=\s*['"]?enabled""")


def code_lines(path):
    try:
        with open(path, errors="replace") as f:
            for n, ln in enumerate(f, 1):
                s = ln.strip()
                if not s or s.startswith(("#", "//", "*", "/*", "\"\"\"")):
                    continue
                yield n, s
    except OSError:
        return


def offenders(root=ROOT):
    out = []
    for d, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS and not x.startswith(".")]
        for fn in files:
            if os.path.splitext(fn)[1] in SKIP_EXT or fn.startswith("test_"):
                continue
            p = os.path.join(d, fn)
            if os.path.islink(p) or os.path.getsize(p) > 4 * 1024 * 1024:
                continue
            for n, s in code_lines(p):
                if UPGRADE.search(s) or FEATURE_SET.search(s):
                    out.append(f"{os.path.relpath(p, root)}:{n}: {s[:160]}")
    return out


class NoAutomaticPoolUpgrade(unittest.TestCase):
    def test_patterns_catch_every_call_form(self):
        for bad in ('zpool upgrade -a', 'run(["zpool", "upgrade", pool])', 'Command::new("zpool").arg("upgrade")',
                    'Command::new("zpool").args(["upgrade", "-a"])', 'zpool set feature@block_cloning=enabled rpool'):
            self.assertTrue(UPGRADE.search(bad) or FEATURE_SET.search(bad), bad)
        for ok in ('zpool import -N rpool', 'zpool status -x', 'zpool get feature@block_cloning_endian rpool', 'emerge --update @world'):
            self.assertFalse(UPGRADE.search(ok) or FEATURE_SET.search(ok), ok)

    def test_repository_never_upgrades_a_pool(self):
        self.assertTrue(os.path.isdir(ROOT), ROOT)
        bad = offenders()
        self.assertEqual(bad, [], "code that upgrades a ZFS pool (pools must keep their feature set; only an admin upgrades by hand):\n" + "\n".join(bad))


if __name__ == "__main__":
    unittest.main()
