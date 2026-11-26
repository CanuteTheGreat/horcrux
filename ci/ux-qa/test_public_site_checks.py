"""Self-tests for the public-site checks (owner-findings rows: Secure Boot claim, GitHub Pages workflow mail)."""
import datetime as dt
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_github_workflows as gw  # noqa: E402
import check_site_claims as sc  # noqa: E402

TODAY = dt.date(2026, 11, 26)


class SiteClaims(unittest.TestCase):
    def dist(self, pages):
        d = tempfile.mkdtemp()
        for rel, html in pages.items():
            p = os.path.join(d, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "w").write(html)
        return d

    def test_unreviewed_secure_boot_claim_fails(self):
        d = self.dist({"index.html": "<ul><li>Secure Boot is not supported.</li><li>ZFS works.</li></ul>"})
        probs, _ = sc.scan(d, [], TODAY)
        self.assertEqual(len(probs), 1)
        self.assertIn("UNREVIEWED", probs[0])
        self.assertIn("Secure Boot", probs[0])

    def test_must_be_off_and_coming_soon_fail(self):
        d = self.dist({"roadmap/index.html": "<p>Secure Boot must be off.</p><p>ARM builds coming soon.</p>"})
        probs, _ = sc.scan(d, [], TODAY)
        self.assertEqual(len(probs), 2)

    def test_reviewed_claim_passes_until_expiry(self):
        d = self.dist({"index.html": "<p>Secure Boot is not supported.</p>"})
        import re
        ok = [(re.compile(r"^/\|Secure Boot is not supported", re.I), "2026-12-20", 1)]
        self.assertEqual(sc.scan(d, ok, TODAY)[0], [])
        expired = [(re.compile(r"^/\|Secure Boot is not supported", re.I), "2026-11-01", 1)]
        self.assertIn("EXPIRED", sc.scan(d, expired, TODAY)[0][0])

    def test_code_blocks_docs_and_404_are_ignored(self):
        d = self.dist({"index.html": "<pre>error: not supported</pre><p>Fine.</p>",
                       "docs/x/index.html": "<p>Not supported yet.</p>", "404.html": "<p>coming soon</p>"})
        self.assertEqual(sc.scan(d, [], TODAY)[0], [])
        self.assertEqual(len(sc.scan(d, [], TODAY, all_pages=True)[0]), 1)

    def test_reviewed_file_needs_reason_and_near_expiry(self):
        f = tempfile.mktemp()
        open(f, "w").write("^/\\|foo\n^/\\|bar  # accurate (until 2027-06-01)\n^/\\|baz  # accurate (until 2026-12-01)\n")
        ok, probs = sc.load_reviewed(f, TODAY)
        self.assertEqual(len(ok), 2)
        self.assertEqual(len(probs), 2)


class GithubWorkflows(unittest.TestCase):
    def repo(self, files, allow=None):
        d = tempfile.mkdtemp()
        os.makedirs(os.path.join(d, ".github/workflows"))
        for n, t in files.items():
            open(os.path.join(d, ".github/workflows", n), "w").write(t)
        if allow is not None:
            os.makedirs(os.path.join(d, "ci/ux-qa"))
            open(os.path.join(d, "ci/ux-qa/github-workflows-allow.txt"), "w").write(allow)
        return d

    def test_pages_push_workflow_fails(self):
        d = self.repo({"pages.yml": "name: p\non:\n  push:\n    branches: [main]\n  workflow_dispatch:\njobs: {}\n"})
        p = gw.check(d)
        self.assertEqual(len(p), 1)
        self.assertIn("push", p[0])

    def test_dispatch_only_passes(self):
        d = self.repo({"windows-check.yml": "on:\n  workflow_dispatch:\n    inputs:\n      package:\n        required: true\njobs: {}\n"})
        self.assertEqual(gw.check(d), [])

    def test_inline_and_list_forms(self):
        self.assertEqual(gw.triggers("on: [push, workflow_dispatch]\n"), {"push", "workflow_dispatch"})
        self.assertEqual(gw.triggers("on: push\n"), {"push"})
        self.assertEqual(gw.triggers("on:\n  - schedule\n  - workflow_dispatch\n"), {"schedule", "workflow_dispatch"})

    def test_allowlist_needs_reason(self):
        wf = {"release.yml": "on:\n  release:\n    types: [published]\n"}
        self.assertEqual(gw.check(self.repo(wf, "release.yml  # signs registry releases\n")), [])
        self.assertEqual(len(gw.check(self.repo(wf, "release.yml\n"))), 2)
        self.assertEqual(gw.check(self.repo(wf, "release.yml  # removal in flight (until 2999-01-01)\n")), [])
        self.assertEqual(len(gw.check(self.repo(wf, "release.yml  # removal in flight (until 2000-01-01)\n"))), 1)


if __name__ == "__main__":
    unittest.main()
