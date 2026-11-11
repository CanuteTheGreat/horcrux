#!/usr/bin/env python3
"""Build a FAKE gringotts backup for the QEMU restore test. No owner data is read: every byte is generated here.

  make-backup-fixture.py OUT_DIR [--restic]

Writes the three archives the restore script expects (same member names/layout as the real backup, old home /home/ron inside the
settings so the path rewrite is exercised):  home-dotfiles.tar.zst, firefox-snap-profile.tar.zst, etc-selected.tar.zst.
With --restic the folder is stored as a snapshot in a throw-away local restic repo (random password, under OUT_DIR/.repo) and OUT_DIR is
refilled by `restic restore`, so the test input is a real `restic restore` output, as it is on crystalcave. The repo is removed afterwards.
"""
import os
import random
import shutil
import subprocess
import sys
import tempfile

OUT = sys.argv[1]
USE_RESTIC = "--restic" in sys.argv[2:]
rnd = random.Random(20261104)


def blob(n):
    return bytes(rnd.getrandbits(8) for _ in range(n))


HOME = {
    ".bashrc": "# fake rc\nexport PATH=$HOME/bin:$PATH\nalias ll='ls -l'\n",
    ".profile": "# fake profile\n",
    ".bash_logout": "# fake\n",
    ".bash_history": "ls /home/ron\ncd /home/ron/Nextcloud\n",
    ".gnupg/pubring.kbx": blob(400),
    ".gnupg/trustdb.gpg": blob(1200),
    ".config/terminator/config": "[global_config]\n[profiles]\n  [[default]]\n    cwd = /home/ron\n",
    ".config/vlc/vlcrc": "# fake vlc\ndvd=/dev/sr0\nrecord-path=/home/ron/Videos\n",
    ".config/remmina/remmina.pref": "[remmina_pref]\nresolution_mode=0\n",
    ".config/remmina/fake.remmina": "[remmina]\nname=fake-host\nserver=192.0.2.10\npassword=\n",
    ".local/share/remmina/group.remmina": "[remmina]\nname=fake\n",
    ".config/discord/Local Storage/leveldb/000003.log": blob(2048),
}
FIREFOX = {
    "snap/firefox/common/.mozilla/firefox/profiles.ini":
        "[Install4F96D1932A9F858E]\nDefault=0fake.default\nLocked=1\n\n[Profile0]\nName=default\nIsRelative=1\nPath=0fake.default\nDefault=1\n\n[General]\nStartWithLastProfile=1\nVersion=2\n",
    "snap/firefox/common/.mozilla/firefox/installs.ini": "[4F96D1932A9F858E]\nDefault=0fake.default\nLocked=1\n",
    "snap/firefox/common/.mozilla/firefox/0fake.default/prefs.js": 'user_pref("browser.download.dir", "/home/ron/Downloads");\n',
    "snap/firefox/common/.mozilla/firefox/0fake.default/places.sqlite": blob(4096),
}
ETC = {
    "etc/nftables-cve-2026-98432.nft": "table inet fake_mitigation {\n  chain input {\n    type filter hook input priority 0; policy accept;\n  }\n}\n",
}


def pack(dst, files, modes=None):
    t = tempfile.mkdtemp(prefix="fixture-")
    try:
        for rel, data in files.items():
            p = os.path.join(t, rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            open(p, "wb" if isinstance(data, bytes) else "w").write(data)
            os.chmod(p, 0o600)
        for rel in ("", ".gnupg", ".config/discord", ".config/remmina", ".local/share/remmina"):
            d = os.path.join(t, rel)
            if rel and os.path.isdir(d):
                os.chmod(d, 0o700)
        # member names without a leading "./": the restore script extracts by name
        subprocess.check_call(["tar", "--zstd", "-cf", dst, "-C", t] + sorted(os.listdir(t)))
    finally:
        shutil.rmtree(t, ignore_errors=True)


def build(d):
    os.makedirs(d, exist_ok=True)
    pack(os.path.join(d, "home-dotfiles.tar.zst"), HOME)
    pack(os.path.join(d, "firefox-snap-profile.tar.zst"), FIREFOX)
    pack(os.path.join(d, "etc-selected.tar.zst"), ETC)


if not USE_RESTIC:
    build(OUT)
else:
    work = tempfile.mkdtemp(prefix="fixture-restic-")
    try:
        src, repo = os.path.join(work, "src"), os.path.join(work, "repo")
        build(src)
        env = dict(os.environ, RESTIC_REPOSITORY=repo, RESTIC_PASSWORD=os.urandom(12).hex())
        subprocess.check_call(["restic", "init", "-q"], env=env)
        subprocess.check_call(["restic", "backup", "-q", "--tag", "fake-fixture", "."], cwd=src, env=env)
        os.makedirs(OUT, exist_ok=True)
        subprocess.check_call(["restic", "restore", "-q", "latest", "--target", OUT], env=env)
        # restic restores with the source path prefix; flatten when it did
        for dp, _, fns in os.walk(OUT):
            if "home-dotfiles.tar.zst" in fns and dp != OUT:
                for f in os.listdir(dp):
                    shutil.move(os.path.join(dp, f), os.path.join(OUT, f))
                break
    finally:
        shutil.rmtree(work, ignore_errors=True)
for f in sorted(os.listdir(OUT)):
    p = os.path.join(OUT, f)
    if os.path.isfile(p):
        print(f, os.path.getsize(p))
