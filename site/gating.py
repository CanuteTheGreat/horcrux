"""Download-variant gating for the product site. Pure logic + one network helper; no third-party imports.

A systemd image is listed only if its .iso AND .iso.sha256 exist on the GitHub `daily` release. The ISO pipeline
uploads assets only after every QEMU test passed, so existence == tested. OpenRC rows are always listed.
"""
import json, os, sys, urllib.request

def release_assets(repo, tag="daily", timeout=15):
    """Asset names of the release, or None if unknown. DL_ASSETS_JSON=<file> (JSON list of names) overrides the
    network (local testing). Never raises."""
    fake = os.environ.get("DL_ASSETS_JSON")
    if fake:
        return set(json.load(open(fake)))
    try:
        req = urllib.request.Request(f"https://api.github.com/repos/CanuteTheGreat/{repo}/releases/tags/{tag}",
                                     headers={"Accept": "application/vnd.github+json", "User-Agent": "site-build"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return {a["name"] for a in json.load(r)["assets"]}
    except Exception as e:  # unreachable API, rate limit, bad JSON: fall back to OpenRC only
        print(f"WARNING: cannot read GitHub release assets ({e}); listing OpenRC variants only", file=sys.stderr)
        return None

def systemd_name(openrc_iso):
    assert "-openrc-" in openrc_iso, openrc_iso
    return openrc_iso.replace("-openrc-", "-systemd-", 1)

def systemd_listed(iso, assets):
    """True only if the systemd twin's .iso and .iso.sha256 are both present in `assets` (None = unknown)."""
    if not assets:
        return False
    s = systemd_name(iso)
    return s in assets and s + ".sha256" in assets

def evidence_name(iso):
    return iso[:-4] + "-test-evidence.tgz"
