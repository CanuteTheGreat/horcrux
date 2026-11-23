"""Permissions of Portage configuration files, whatever the caller's umask.

Anything under /etc/portage (except gnupg/, which holds keys) must stay readable and traversable by every user: administrators run
`emerge -p`, `equery` and `eix` as themselves (portage group), and Portage's unprivileged build user reads repos.conf and package.*.
A writer running with umask 077 (an installer service, an ssh login, an agent shell) used to leave 0600 files and 0700 directories
there. The same rule is in horcrux_profile.py (single-file program, so it carries its own copy; test_portage_writers.py keeps both honest).
"""
import os

CONFIG_DIR_MODE = 0o755
CONFIG_FILE_MODE = 0o644


def _parts(path):
    return [p for p in os.path.normpath("/" + str(path).lstrip("/")).split("/") if p]


def is_config(path):
    """True for /etc/portage itself and everything below it except the gnupg keyring directory"""
    p = _parts(path)
    return p[:2] == ["etc", "portage"] and p[2:3] != ["gnupg"]


def config_mode(path, mode=CONFIG_FILE_MODE):
    """the mode a file at `path` gets: portage config is at least world-readable and never group/world-writable"""
    if not is_config(path):
        return mode
    return (mode | 0o444) & ~0o022


def ensure_dirs(directory, root="/"):
    """makedirs for a portage config directory: every directory it creates is 0755 (os.makedirs' mode is filtered by the umask)"""
    if not is_config(os.path.relpath(directory, root) if root != "/" else directory):
        os.makedirs(directory, exist_ok=True)
        return
    missing = []
    d = directory
    while d and not os.path.isdir(d):
        missing.append(d)
        nd = os.path.dirname(d)
        if nd == d:
            break
        d = nd
    for d in reversed(missing):
        try:
            os.mkdir(d, CONFIG_DIR_MODE)
        except FileExistsError:
            pass
        os.chmod(d, CONFIG_DIR_MODE)
