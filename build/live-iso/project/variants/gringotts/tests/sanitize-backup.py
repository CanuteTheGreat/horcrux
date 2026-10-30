#!/usr/bin/env python3
"""Make a SANITIZED STAND-IN of ~/Nextcloud/gringotts-backup-2026-10 for the QEMU restore test.

The real backup holds personal data (browser logins, Discord tokens, history) and never leaves crystalcave/Nextcloud. The stand-in keeps the
exact archive layout (same member names, modes, directories, symlinks, sizes capped) so the restore script is exercised on the real structure,
but every file's content is replaced by random bytes, except a small allow-list of harmless text settings (shell rc files, terminator/vlc/
remmina prefs, nftables ruleset, profiles.ini) and synthetic Preferences / history files that contain the old home path /home/ron.

  sanitize-backup.py SRC_DIR DST_DIR     (SRC_DIR = the real backup folder; DST_DIR is created)
"""
import os
import random
import shutil
import subprocess
import sys
import tarfile
import tempfile

SRC, DST = sys.argv[1], sys.argv[2]
random.seed(20261029)
KEEP_REAL = {".bashrc", ".profile", ".bash_logout", ".config/terminator/config", ".config/vlc/vlcrc", ".config/vlc/vlc-qt-interface.conf",
             ".config/remmina/remmina.pref", ".config/virt-viewer/settings", ".config/gtk-3.0/bookmarks",
             "snap/firefox/common/.mozilla/firefox/profiles.ini", "snap/firefox/common/.mozilla/firefox/installs.ini",
             "etc/nftables-cve-2026-98432.nft"}
SYNTH = {
    ".config/BraveSoftware/Brave-Browser/Default/Preferences": '{"download":{"default_directory":"/home/ron/Downloads"},"homepage":"https://example.invalid/","profile":{"name":"Person 1"}}\n',
    ".bash_history": "ls /home/ron\ncd /home/ron/Nextcloud\nsudo apt update\n",
    ".viminfo": "# synthetic\n'0  1  0  /home/ron/notes.txt\n",
    "snap/firefox/common/.mozilla/firefox/0dtt0h6y.default/prefs.js": 'user_pref("browser.download.dir", "/home/ron/Downloads");\n',
}
CAP = 200_000


def rand_bytes(n):
    n = min(n, CAP)
    return bytes(random.getrandbits(8) for _ in range(min(n, 4096))) * (n // 4096 + 1)


def sanitize(arc_in, arc_out):
    tmp = tempfile.mkdtemp(prefix="sanitize-")
    try:
        subprocess.check_call(["tar", "--zstd", "-xf", arc_in, "-C", tmp, "--same-permissions", "--no-same-owner"])
        subprocess.check_call(["chmod", "-R", "u+rwX", tmp])
        n_keep = n_syn = n_rand = 0
        for dp, dns, fns in os.walk(tmp):
            for fn in fns:
                p = os.path.join(dp, fn)
                rel = os.path.relpath(p, tmp)
                if os.path.islink(p):
                    continue
                size = os.path.getsize(p)
                mode = os.stat(p).st_mode & 0o7777
                if rel in KEEP_REAL and size < 300_000:
                    n_keep += 1
                    continue
                if rel in SYNTH:
                    open(p, "w").write(SYNTH[rel])
                    n_syn += 1
                else:
                    open(p, "wb").write(rand_bytes(size))
                    n_rand += 1
                os.chmod(p, mode)
        # non-KEEP files must not leak names with secrets: names stay (layout fidelity); contents are random
        for name in SYNTH:                      # make sure the synthetic files exist even if the real archive lacks them
            p = os.path.join(tmp, name)
            if not os.path.exists(p) and any(name.startswith(x) for x in (".bash", ".viminfo", ".config/Brave")):
                os.makedirs(os.path.dirname(p), exist_ok=True)
                open(p, "w").write(SYNTH[name])
        subprocess.check_call("tar --zstd -cf %s -C %s ." % (arc_out, tmp), shell=True)
        return n_keep, n_syn, n_rand
    finally:
        subprocess.call(["chmod", "-R", "u+rwX", tmp])
        shutil.rmtree(tmp, ignore_errors=True)


os.makedirs(DST, exist_ok=True)
for name in ("home-dotfiles.tar.zst", "firefox-snap-profile.tar.zst", "etc-selected.tar.zst"):
    src = os.path.join(SRC, name)
    if not os.path.exists(src):
        print("missing", src)
        continue
    k, s, r = sanitize(src, os.path.join(DST, name))
    print(f"{name}: kept-real {k}, synthetic {s}, random {r}; {os.path.getsize(os.path.join(DST, name))} bytes")
