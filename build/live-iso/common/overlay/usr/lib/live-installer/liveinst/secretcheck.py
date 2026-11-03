"""The disk passphrase is ONLY for the disk.

Rules enforced for every front end (guided GUI, text installer, answer file, the per-machine auto installers):
  * no account (desktop user, root, web admin) may use the disk passphrase as its password;
  * root and the desktop user must not share a password either (root is locked by default, sudo goes through wheel).
Everything here compares in memory: a plain-text password is compared as a string, a crypt hash is verified by hashing
the passphrase with the hash's own salt through libcrypt (or the `crypt` module) and comparing. Nothing is written
to disk, to a log or to an argument vector (openssl fallback reads the secret from stdin).
`hash_verifies` is also what the QEMU gates use on the installed /etc/shadow with the known TEST passphrase.
"""
import ctypes
import ctypes.util
import hmac
import re
import subprocess

DISK_REUSE = ("This is the disk passphrase. The disk passphrase is only for unlocking the disk: "
              "choose a different password for the account.")
_HASH_RE = re.compile(r"^\$(?:1|5|6|y|2[aby]|gy|7)\$")


def _crypt_fn():
    try:
        import crypt as _c          # Python < 3.13
        return lambda pw, setting: _c.crypt(pw, setting)
    except Exception:               # noqa
        pass
    for name in ("libcrypt.so.2", "libcrypt.so.1", ctypes.util.find_library("crypt") or "libcrypt.so"):
        try:
            lib = ctypes.CDLL(name)
        except OSError:
            continue
        lib.crypt.restype = ctypes.c_char_p
        lib.crypt.argtypes = [ctypes.c_char_p, ctypes.c_char_p]

        def fn(pw, setting, _lib=lib):
            r = _lib.crypt(pw.encode(), setting.encode())
            return r.decode() if r else None
        return fn
    return None


def _openssl_verify(pw, h):
    m = re.match(r"^\$(1|5|6)\$(?:rounds=\d+\$)?([^$]{1,16})\$", h)
    if not m or "rounds=" in h:
        return None
    try:
        out = subprocess.run(["openssl", "passwd", "-" + m.group(1), "-salt", m.group(2), "-stdin"],
                             input=pw + "\n", capture_output=True, text=True, timeout=20)
    except (OSError, subprocess.SubprocessError):
        return None
    return hmac.compare_digest(out.stdout.strip(), h) if out.returncode == 0 else None


def hash_verifies(pw, h):
    """True/False when the crypt hash `h` is (not) the hash of `pw`; None when it cannot be checked here
    (locked/empty entry, unknown scheme, no crypt library)."""
    if not h or not _HASH_RE.match(h):
        return None
    fn = _crypt_fn()
    if fn is not None:
        try:
            r = fn(pw, h)
        except Exception:           # noqa
            r = None
        if r and r != "*0" and r != "*1":
            return hmac.compare_digest(r, h)
    return _openssl_verify(pw, h)


def disk_passphrase(c):
    if c.get("fs") == "xfs-luks":
        return c.get("luks_passphrase") or ""
    if c.get("zfs_encrypt"):
        return c.get("zfs_passphrase") or ""
    return ""


def _accounts(c):
    d = c.get("desktop") or {}
    out = []
    if c.get("user_name") or c.get("user_password") or c.get("user_hash"):
        out.append(("the user account " + (c.get("user_name") or ""), c.get("user_password", ""), c.get("user_hash", "")))
    if d.get("enable") or d.get("password") or d.get("password_hash"):
        out.append(("the desktop user " + (d.get("user") or ""), d.get("password", ""), d.get("password_hash", "")))
    out.append(("root", c.get("root_password", ""), c.get("root_hash", "")))
    out.append(("the web admin", c.get("web_password", ""), ""))
    return out


def _same(secret, plain, h):
    """True = the account's password IS `secret`; None = cannot tell (hash in an unknown scheme)."""
    if plain:
        return hmac.compare_digest(plain.encode(), secret.encode())
    if h:
        return hash_verifies(secret, h)
    return False


def reuse_problems(c, unverifiable=None):
    """Sentences describing every reuse; empty list = fine. `unverifiable(label)` is called for a hash that could not be checked."""
    errs = []
    disk = disk_passphrase(c)
    acc = _accounts(c)
    if disk:
        for label, plain, h in acc:
            r = _same(disk, plain, h)
            if r:
                errs.append(f"{label}: {DISK_REUSE}")
            elif r is None and unverifiable:
                unverifiable(label)
    # root vs the desktop/user account: root stays locked by default; when it is set it must be its own password
    root = next((a for a in acc if a[0] == "root"), None)
    if root and (root[1] or root[2]):
        for label, plain, h in acc:
            if label == "root" or label == "the web admin":
                continue
            if plain and root[1] and hmac.compare_digest(plain.encode(), root[1].encode()):
                errs.append(f"root and {label} must not share a password (root is locked unless you set a separate password).")
            elif plain and root[2] and hash_verifies(plain, root[2]):
                errs.append(f"root and {label} must not share a password (root is locked unless you set a separate password).")
    return errs


def entry_problem(candidate, c, what="password"):
    """For the interactive forms: `candidate` typed for an account vs the disk passphrase already in `c`."""
    disk = disk_passphrase(c)
    if disk and candidate and hmac.compare_digest(candidate.encode(), disk.encode()):
        return DISK_REUSE
    return None
