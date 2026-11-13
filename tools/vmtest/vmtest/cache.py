"""Disk cache: provision once, rerun in seconds.

    base = cache.ensure("horcrux-installed-<isosha>", build)   # build(path) writes a qcow2 on first use
    NodeSpec(name="n1", image=base)                            # every run gets a throw-away overlay on top

The cache key must contain everything the image depends on (ISO sha, answer file, harness version);
a stale key is a stale image.  `VMTEST_CACHE` overrides the location (default ~/.cache/vmtest).
"""
from __future__ import annotations

import hashlib
import os
import shutil
import time


def cache_dir():
    d = os.environ.get("VMTEST_CACHE") or os.path.join(os.path.expanduser("~"), ".cache", "vmtest")
    os.makedirs(d, exist_ok=True)
    return d


def sha256_file(path, limit=None):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        n = 0
        while True:
            b = f.read(1 << 20)
            if not b or (limit and n >= limit):
                break
            h.update(b)
            n += len(b)
    return h.hexdigest()


def ensure(key: str, build, suffix=".qcow2"):
    """Return the cached file for `key`, building it with build(tmp_path) when missing (atomic rename)."""
    path = os.path.join(cache_dir(), key + suffix)
    if os.path.exists(path):
        os.utime(path)
        return path
    tmp = path + f".tmp{os.getpid()}"
    try:
        build(tmp)
        os.rename(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)
    return path


def prune(max_age_days=14, max_total_gb=60):
    """Drop entries unused for max_age_days, then oldest-first until under max_total_gb."""
    d = cache_dir()
    files = sorted((os.path.join(d, f) for f in os.listdir(d)), key=os.path.getmtime)
    now = time.time()
    for f in list(files):
        if now - os.path.getmtime(f) > max_age_days * 86400:
            os.unlink(f)
            files.remove(f)
    while files and sum(os.path.getsize(f) for f in files) > max_total_gb * 2**30:
        os.unlink(files.pop(0))


def fetch(url: str, sha256: str, name: str | None = None):
    """Download `url` into the cache once, verify its sha256 (a mismatch deletes the file and raises)."""
    import subprocess
    path = os.path.join(cache_dir(), name or os.path.basename(url))
    if os.path.exists(path) and sha256_file(path) == sha256:
        return path
    tmp = path + f".tmp{os.getpid()}"
    subprocess.check_call(["curl", "-fsSL", "--retry", "3", "-o", tmp, url])
    got = sha256_file(tmp)
    if got != sha256:
        os.unlink(tmp)
        raise RuntimeError(f"{url}: sha256 {got} != pinned {sha256}")
    os.rename(tmp, path)
    return path
