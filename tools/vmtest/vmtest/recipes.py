"""Reusable multi-step procedures for the two products: unattended install from the live ISO.

Shared by install_smoke.py and every scenario that needs an installed Horcrux/Patronus node.
"""
from __future__ import annotations

import os
import subprocess
import tempfile
import time

PW = "Kx9!mQv2-tle-77Zp"          # >= 12 chars, mixed: passes the installer's strength rules
PROJECTS = {
    "horcrux": dict(svc="horcrux-api", port=8006, web_user="hxadmin", health="/api/health"),
    "patronus": dict(svc="patronus-web", port=8443, web_user="pxadmin", health="/login"),
}


def answer_toml(project, pubkey, pwhash, lan_ip="10.77.0.1", fs="ext4"):
    p, P = project, PROJECTS[project]
    L = ["[global]", 'keyboard = "us"', 'timezone = "Europe/Berlin"', f'hostname = "{p}-vmt.test.lan"', 'email = "ops@example.test"',
         f'root_password_hash = "{pwhash}"', f'ssh_keys = ["{pubkey}"]', "ssh_password_auth = false", "console_menu_open = false",
         "", "[disk]", f'filesystem = "{fs}"', 'disks = ["/dev/vda"]', "", "[user]", f'name = "{P["web_user"]}"', f'password_hash = "{pwhash}"', "",
         "[hardening]", "ssh = true", "sysctl = true", "", "[use_flags]", 'preset = "minimal"', ""]
    L += ["[network.management]", 'mode = "dhcp"'] if p == "horcrux" else ["[network.wan]", 'mode = "dhcp"', "", "[network.lan]", f'address = "{lan_ip}/24"']
    return "\n".join(L) + "\n"


def make_answers(project, work=None, **kw):
    """-> (answers.img path, ssh private key path, root/web password). ssh key is authorised for root by the answer file."""
    work = work or tempfile.mkdtemp(prefix="vmt-ans-")
    key = os.path.join(work, "key")
    subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "vmtest", "-f", key])
    pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "vmtest01", PW]).decode().strip()
    d = os.path.join(work, "a")
    os.mkdir(d)
    open(f"{d}/answer.toml", "w").write(answer_toml(project, open(key + ".pub").read().strip(), pwhash, **kw))
    img = os.path.join(work, "answers.img")
    subprocess.check_call(["mke2fs", "-q", "-t", "ext4", "-L", f"{project.upper()}-ANSWERS", "-d", d, img, "8M"])
    return img, key


def unattended_install(m, n, install_timeout=1700):
    """Drive GRUB (serial submenu, automated entry), wait for AUTOINSTALL-OK and for the VM to power itself off.
    Then swap to the installed disk (eject media, fresh NVRAM) so the next n.start() is the first boot."""
    q = n.start().q
    m.check(f"{n.name}:grub-menu-reached", q.expect(r"Serial console \(ttyS0", n.T(120)), "")
    time.sleep(2)
    q.send("s")                       # serial-console submenu, then 4 = automated install
    time.sleep(1.5)
    q.send("4")
    m.check(f"{n.name}:kernel-boot-started", q.expect(r"(Linux version|Booting|OpenRC|systemd)", n.T(90)), "")
    m.check(f"{n.name}:live-userland-up", q.expect(r"AUTOINSTALL-START", n.T(420)), "answer file found")
    bad = q.expect(r"(ANSWER-FILE-ERROR[^\n]*|No answer file found[^\n]*)", 3, consume=False)
    m.check(f"{n.name}:answer-file-accepted", not bad, bad.group(0).decode() if bad else "validated")
    r = q.expect(r"AUTOINSTALL-(OK|FAILED[^\n]*)", n.T(install_timeout), consume=False)
    m.check(f"{n.name}:installer-finished", r and b"AUTOINSTALL-OK" in r.group(0), r.group(0).decode() if r else "timeout")
    m.check(f"{n.name}:vm-stopped-after-install", q.wait_exit(n.T(180)), "installer powers the VM off")
    n.eject_media()
    n.reset_nvram()
