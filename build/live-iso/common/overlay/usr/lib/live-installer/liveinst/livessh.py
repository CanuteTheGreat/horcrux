"""Key-only sshd for the LIVE system of an unattended install (`<proj>.autoinstall.after=reboot`, the lab / fleet flow).

Why: an unattended reinstall that does not come back leaves nobody to ask what happened. gringotts (2026-11-07) answered ping at a new DHCP address for
hours with every TCP port closed (the live system: sshd is off there by design) and its install log sat on a disk nobody could read. With the
operator's own keys from the answer file the live system stays inspectable (`ssh root@host cat /var/log/autoinstall.log`) and can be restarted.

Everything lives in /run (tmpfs): nothing of it reaches the installed system (the installer copies the live root, never /run), no host key is
shared, nothing persists. Without an ssh key in the answer file no sshd is started (the live system stays closed, as before).
"""
import os
import re
import subprocess
import sys

try:
    import tomllib
except ImportError:                                     # pragma: no cover (python < 3.11)
    tomllib = None

RUN = "/run/live-autoinstall-ssh"
KEY_RE = re.compile(r"^(ssh-(ed25519|rsa|dss)|ecdsa-sha2-\S+|sk-(ssh-ed25519|ecdsa-sha2-\S+)@openssh\.com)\s+\S+")


def parse_keys(text):
    """the operator's public keys from an answer file's [global] ssh_keys / ssh_key (pure)"""
    if tomllib is None:
        return []
    try:
        g = tomllib.loads(text).get("global", {})
    except Exception:
        return []
    keys = g.get("ssh_keys", g.get("ssh_key", []))
    keys = [keys] if isinstance(keys, str) else list(keys or [])
    out = []
    for k in keys:
        for line in str(k).splitlines():
            line = line.strip()
            if KEY_RE.match(line):
                out.append(line)
    return out


def sshd_config(run=RUN, port=22):
    """pure: key-only, root only, every file in /run"""
    return ("# live system of an unattended install: key-only, the operator's keys from the answer file, nothing persistent\n"
            f"Port {port}\nPidFile {run}/sshd.pid\nHostKey {run}/ssh_host_ed25519_key\nAuthorizedKeysFile {run}/authorized_keys\n"
            "PermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\n"
            "AuthenticationMethods publickey\nPermitEmptyPasswords no\nStrictModes no\nUsePAM yes\nX11Forwarding no\nLogLevel INFO\n")


def start(answers_text, run=RUN, port=22, sshd="/usr/sbin/sshd", keygen="/usr/bin/ssh-keygen", runner=subprocess.run, use_pam=True):
    """returns (started, message)"""
    keys = parse_keys(answers_text)
    if not keys:
        return False, "no ssh key in the answer file: the live system stays closed (sshd off)"
    os.makedirs(run, exist_ok=True)
    os.chmod(run, 0o755)                                # sshd's privsep user must traverse it; the private host key stays 0600
    with open(f"{run}/authorized_keys", "w") as f:
        f.write("\n".join(keys) + "\n")
    os.chmod(f"{run}/authorized_keys", 0o644)
    hk = f"{run}/ssh_host_ed25519_key"
    if not os.path.exists(hk):
        r = runner([keygen, "-q", "-t", "ed25519", "-N", "", "-f", hk], capture_output=True, text=True)
        if r.returncode != 0:
            return False, "ssh-keygen failed: " + (r.stderr or "")[:200]
    cfg = sshd_config(run, port)
    if not use_pam:
        cfg = cfg.replace("UsePAM yes", "UsePAM no")
    with open(f"{run}/sshd_config", "w") as f:
        f.write(cfg)
    for d in ("/var/empty", "/run/sshd"):               # sshd privsep chroot: /var/empty on Gentoo (present on the live image), /run/sshd on Debian/Ubuntu builds (a tmpfs path, so still nothing persistent)
        try:
            os.makedirs(d, exist_ok=True)
        except OSError:
            pass
    r = runner([sshd, "-f", f"{run}/sshd_config"], capture_output=True, text=True)
    if r.returncode != 0:
        return False, "sshd failed to start: " + (r.stderr or "")[:200]
    return True, f"live sshd up on port {port}, key-only, {len(keys)} key(s) from the answer file"


def main(argv=None):
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from . import system as S
    lab = "/etc/live-iso-label"
    proj = open(lab).read().strip().removesuffix("-live") if os.path.exists(lab) else "horcrux"
    path = S.find_answer_file(proj, open("/proc/cmdline").read())
    if not path:
        print("live-autoinstall-ssh: no answer file, sshd stays off")
        return 0
    ok, msg = start(open(path).read())
    print("live-autoinstall-ssh: " + msg)
    return 0


if __name__ == "__main__":
    sys.exit(main())
