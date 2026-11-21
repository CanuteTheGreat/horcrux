#!/usr/bin/env python3
"""QEMU gate: ONE stock installer ISO + a MACHINE PROFILE = the machine (no per-machine image build).

  machine-profile-test.py --iso <stock ISO> --out DIR --profile <bundle dir with profile.toml> [--desktop] [--fake-t2 --mac-esp ESP.img]
                          [--dev-overlay <repo gentoo/ dir>] [--private-overlay <dir>]

Flow (UEFI, USB stick, answer file carries only the secrets the harness types; everything about the machine comes from the profile):
  1. the profile bundle is put on the answer volume and named by the answer file ([profile] source); the installer installs the base
     from the ISO, then runs horcrux-profile inside the new system: detected hardware layers + the profile, from BINARY packages
     (a source build outside the trusted script/config repos fails the install). Install wall time is measured.
  2. --fake-t2: QEMU cannot present the T2 bridge, so the answer file forces t2 = yes and t2_kernel = skip (TEST ONLY: no T2 kernel, no
     apple-bce initramfs gate); a second disk carries a Mac EFI partition with t2linux's firmware-raw.tar.gz made from a SYNTHETIC macOS
     firmware tree (no Apple data). Asserted: the apple-t2 layer packages are selected and installed, the firmware is converted and
     installed as the package sys-firmware/apple-t2-firmware-local (package-owned), the T2 dracut/modprobe config is written.
  3. boot #1: disk passphrase on the serial console, then over ssh: host name, edition, generation 1 recorded, every profile/layer
     package installed and in @world, administrator in wheel+portage+profile groups (equery/eix as that user), Wi-Fi network file,
     profile files, nothing personal in a generic profile, `horcrux-profile plan` clean, GDM greeter up with no user session (desktop),
     a real PAM login of the profile's administrator.
  4. declarative change: add a package to the profile, `horcrux-profile apply` = generation 2 with the package; remove it again =
     generation 3, package deselected from @world.
Evidence in --out (result.json, serial logs, timings.json).
"""
import argparse
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("zfs_enc_test", os.path.join(HERE, "zfs-enc-test.py"))
ZE = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ZE)
qt = ZE.qt
sys.path.insert(0, os.path.join(HERE, "overlay", "usr", "lib", "live-installer"))
import horcrux_profile as E  # noqa: E402

EDIT_PKG = "app-misc/screen"        # a small package with a binary on the binhosts, used for the declarative add/remove check


def build_bundle(src, dst, dev_overlay=None, private_overlay=None):
    """copy the profile bundle; add the branch's overlay as repo horcrux-dev (pre-merge: the published overlay does not have the new
    packages yet) and, when the profile names it, a snapshot of the private overlay"""
    shutil.copytree(src, dst)
    extra = ""
    if dev_overlay:
        r = os.path.join(dst, "repos", "horcrux-dev")
        shutil.copytree(dev_overlay, r, ignore=shutil.ignore_patterns(".git", "ci", "*.pyc"))
        with open(os.path.join(r, "profiles", "repo_name"), "w") as f:
            f.write("horcrux-dev\n")
        lc = os.path.join(r, "metadata", "layout.conf")
        txt = re.sub(r"(?m)^repo-name\s*=.*$", "repo-name = horcrux-dev", open(lc).read())
        if "masters" not in txt:
            txt += "masters = gentoo\n"
        open(lc, "w").write(txt)
        extra += ('\n[[repos]]\nname = "horcrux-dev"\nsnapshot = "bundle:repos/horcrux-dev"\npriority = 100\nsource_ok = true\n'
                  '\n[portage.accept_keywords."*/*::horcrux-dev"]\n')
    prof = open(os.path.join(dst, "profile.toml")).read()
    if private_overlay and "bundle:repos/ctg-private" in prof:
        subprocess.check_call(f"git -C {private_overlay} archive HEAD | tar -C {os.path.join(dst, 'repos')} --one-top-level=ctg-private -x", shell=True) \
            if os.path.isdir(os.path.join(private_overlay, ".git")) else shutil.copytree(private_overlay, os.path.join(dst, "repos", "ctg-private"))
    if extra:
        # accept_keywords for the dev overlay is merged into the profile's own [portage.accept_keywords] table
        import tomllib
        mp = tomllib.loads(prof)
        kw = mp.get("portage", {}).get("accept_keywords", {})
        prof += ('\n[[repos]]\nname = "horcrux-dev"\nsnapshot = "bundle:repos/horcrux-dev"\npriority = 100\nsource_ok = true\n')
        if kw:
            prof = prof.replace("[portage.accept_keywords]", '[portage.accept_keywords]\n"*/*::horcrux-dev" = "~amd64"', 1)
        else:
            prof += '\n[portage.accept_keywords]\n"*/*::horcrux-dev" = "~amd64"\n'
        open(os.path.join(dst, "profile.toml"), "w").write(prof)
    return E.load(os.path.join(dst, "profile.toml"))


class PRun(ZE.ZRun):
    def __init__(self, a, mp):
        z = argparse.Namespace(iso=a.iso, out=a.out, desktop=a.desktop, server=not a.desktop, public=False, overlay_dir="", init="", late_network=0,
                               hibernate=False, fake_t2=a.fake_t2, expect_t2_refusal=False, t2="yes" if a.fake_t2 else "no", rtc_base="",
                               mem=a.mem, cpu=a.cpu, disk_gb=a.disk_gb, hibernate_mem=0, keep_disks=a.keep_disks)
        super().__init__(z)
        self.pa, self.mp = a, mp
        self.user = next((u["name"] for u in mp.get("users", []) if u.get("admin", True)), "owner")
        self.timings = {}

    def qemu_args(self, iso, installed, nvars):
        args = super().qemu_args(iso, installed, nvars)
        if iso and self.pa.mac_esp:          # the Mac's own disk with its EFI partition: only present while the installer runs
            args += ["-drive", f"file={self.pa.mac_esp},if=virtio,format=raw,readonly=on"]
        return args

    def wait_installer(self, vm, timeout, *rest):
        # base install + hardware layers + the profile's packages (binary): the profile part is a download-bound emerge
        return super().wait_installer(vm, timeout * 2, *rest)

    def toml(self, pubkey, pwhash):
        t = super().toml(pubkey, pwhash)
        t = t.replace('t2 = "yes"', 't2 = "yes"\nt2_kernel = "skip"') if self.pa.fake_t2 else t
        return t + '\n[profile]\nsource = "/run/answers-media/profile"\n'

    def run(self):
        a = self.z
        os.makedirs(a.out, exist_ok=True)
        self.disk, self.disk2, self.answers = f"{a.out}/target.qcow2", f"{a.out}/target2.qcow2", f"{a.out}/answers.img"
        nvars = f"{a.out}/OVMF_VARS.fd"
        subprocess.check_call(["qemu-img", "create", "-q", "-f", "qcow2", self.disk, f"{a.disk_gb}G"])
        qt.pflash_vars(nvars)
        self.key = f"{a.out}/test_key"
        for f in (self.key, self.key + ".pub"):
            if os.path.exists(f):
                os.unlink(f)
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "qemu-test", "-f", self.key])
        self.pub = open(self.key + ".pub").read().strip()
        pwhash = subprocess.check_output(["openssl", "passwd", "-6", "-salt", "qemutest1", qt.PW]).decode().strip()
        d = tempfile.mkdtemp(dir=a.out)
        open(f"{d}/answer.toml", "w").write(self.toml(self.pub, pwhash))
        shutil.copytree(self.pa.bundle, f"{d}/profile")
        subprocess.check_call(["mke2fs", "-q", "-t", "ext4", "-L", "HORCRUX-ANSWERS", "-d", d, self.answers, "512M"])
        shutil.rmtree(d)
        self.step("answer-file-and-profile-bundle-built", True, f"profile {self.mp['machine']['hostname']} edition={self.mp['machine'].get('edition')}; "
                  "the answer file carries only the harness's secrets")
        t0 = time.time()
        self.phase1(nvars)
        self.timings["install_s"] = round(time.time() - t0)
        log = open(f"{a.out}/1-install-serial.log", errors="replace").read()
        m = re.search(r"machine profile applied in (\d+) s \(layers: ([^)]*)\)", log)
        self.timings["profile_apply_s"] = int(m.group(1)) if m else None
        self.step("install-under-30-min-from-stock-iso", self.timings["install_s"] < 1800, f"{self.timings['install_s']} s total, profile+layers {self.timings['profile_apply_s']} s, 0 image builds")
        self.step("profile-announced-and-applied-by-the-installer", "MACHINE-PROFILE " + self.mp["machine"]["hostname"] in log and m is not None,
                  (m.group(0) if m else "no 'machine profile applied' line") + " | " + "; ".join(re.findall(r"hardware support layers: [^\n]*", log))[:200])
        self.step("no-source-build-outside-trusted-repos", "would be compiled on this machine" not in log, "binary packages only (ebuild installs only for script/config repos)")
        if self.pa.fake_t2:
            self.step("t2-firmware-captured-from-the-mac-efi-partition-before-the-disk-was-touched",
                      re.search(r"Apple firmware: \d+ files from raw /run/mac-esp", log) is not None, "; ".join(re.findall(r"Apple firmware: [^\n]*", log))[:240])
        self.boot1(nvars)
        self.res["timings"] = self.timings
        json.dump(self.timings, open(f"{a.out}/timings.json", "w"), indent=1)
        self.res["ok"] = not getattr(self, "failed", [])
        json.dump(self.res, open(f"{a.out}/result.json", "w"), indent=1)
        if getattr(self, "failed", []):
            print("FAILED STEPS:", ", ".join(self.failed))
            sys.exit(1)
        print("ALL PASS machine-profile", self.mp["machine"]["hostname"], json.dumps(self.timings))

    def boot1(self, nvars):
        a = self.z
        qt.pflash_vars(nvars)
        vm = qt.VM("2-boot1", self.qemu_args(None, True, nvars), a.out)
        try:
            m = vm.expect_raw(r"(?i)(encrypted zfs password|enter passphrase|password for rpool|disk passphrase)[^\n]*", 420)
            if self.mp.get("disk", {}).get("encryption", True):
                self.step("boot1-passphrase-prompt", m, m.group(0).decode(errors="replace").strip() if m else "no prompt")
                time.sleep(1)
                vm.send(ZE.PASSPHRASE + "\r")
            self.step("boot1-ssh-after-passphrase", self.wait_ssh(), "system came up after the passphrase")
            self.profile_checks()
            if a.desktop:
                self.desktop_profile_checks(vm)
            self.declarative_change_checks()
            self.sh("poweroff", timeout=10)
            self.wait_exit(vm)
        finally:
            if vm.alive():
                vm.p.kill()

    def profile_checks(self):
        mp, u = self.mp, self.user
        rc, o = self.sh("hostname; . /usr/lib/os-release; echo VARIANT=$VARIANT_ID; cat /var/lib/horcrux/profile/current; ls /var/lib/horcrux/profile/generations")
        self.step("hostname-from-the-profile", o.splitlines()[:1] == [mp["machine"]["hostname"]], o.splitlines()[0] if o else "")
        try:
            gen = json.loads(o[o.index("{"):o.rindex("}") + 1])
        except ValueError:
            gen = {}
        self.step("generation-1-recorded-at-install", gen.get("generation") == 1 and gen.get("install") is True, re.sub(r"\s+", " ", o)[:300])
        want = list(gen.get("packages", []))
        self.res["profile_packages"] = want
        script = "for a in " + " ".join(want) + "; do case $a in @*) grep -qx \"${a}\" /var/lib/portage/world_sets && echo \"OK $a\" || echo \"MISS $a\";; " \
                 "*) if portageq has_version / \"$a\" && grep -qx \"$a\" /var/lib/portage/world; then echo \"OK $a\"; else echo \"MISS $a\"; fi;; esac; done"
        rc, o = self.sh(script, timeout=180)
        miss = re.findall(r"MISS (\S+)", o)
        self.step("profile-and-layer-packages-installed-and-in-world", want and not miss, f"{len(want)} packages; missing: {miss or 'none'}")
        rc, o = self.sh("qlist -IC sys-apps/horcrux-profile; command -v horcrux-profile; horcrux-profile generations")
        self.step("horcrux-profile-is-package-owned-and-lists-generations", "sys-apps/horcrux-profile" in o and "/usr/bin/horcrux-profile" in o and "(install)" in o, re.sub(r"\s+", " ", o)[:300])
        groups = E.user_groups(mp).get(u, [])
        rc, o = self.sh(f"id -nG {u}; getent group | cut -d: -f1 | tr '\\n' ' '")
        have = o.splitlines()[0].split() if o else []
        existing = set(o.splitlines()[1].split()) if len(o.splitlines()) > 1 else set()
        miss = [g for g in groups if g in existing and g not in have]
        self.step("administrator-in-wheel-portage-and-profile-groups", "wheel" in have and "portage" in have and not miss, f"{u}: {' '.join(have)}; missing: {miss or 'none'}")
        qt.HG.portage_group_gate(self.sh, self.step, u, wait=int(qt.T(300)))
        for w in mp.get("wifi", []):
            rc, o = self.sh(f"stat -c '%a %U' '/etc/NetworkManager/system-connections/{re.sub(r'[^A-Za-z0-9_.-]', '_', w['ssid'])}.nmconnection'")
            self.step(f"wifi-network-{w['ssid']}-written-root-only", o.strip() == "600 root", o.strip())
        for f in mp.get("files", []):
            rc, o = self.sh(f"test -f '{f['path']}' && echo PRESENT")
            self.step(f"profile-file-{f['path']}", "PRESENT" in o, o.strip())
        rc, o = self.sh("horcrux-profile plan", timeout=120)
        self.step("plan-after-install-has-no-package-or-file-changes", rc == 0 and not re.search(r"(?m)^[+~-] (package|file) ", o), re.sub(r"\s+", " ", o)[:300])
        if self.pa.fake_t2:
            rc, o = self.sh("for p in sys-kernel/horcrux-t2-config sys-apps/tiny-dfr sys-apps/t2fanrd sys-apps/horcrux-t2-audio sys-firmware/apple-t2-firmware-local; do "
                            "qlist -IC $p >/dev/null && echo \"OK $p\" || echo \"MISS $p\"; done; "
                            "ls /lib/firmware/brcm | grep -c apple; qfile -q /lib/firmware/brcm/brcmfmac4364b2-pcie.apple,hanauma.bin; "
                            "test -f /etc/dracut.conf.d/90-horcrux-t2.conf && echo DRACUT; test -f /etc/modprobe.d/90-horcrux-t2.conf && echo MODPROBE")
            self.step("t2-layer-packages-installed", "MISS" not in o and o.count("OK ") == 5, re.sub(r"\s+", " ", o)[:300])
            self.step("t2-firmware-package-owned-incl-hard-linked-names", "sys-firmware/apple-t2-firmware-local" in o, re.sub(r"\s+", " ", o)[:300])
            self.step("t2-dracut-and-modprobe-config-written", "DRACUT" in o and "MODPROBE" in o, "")
        else:
            rc, o = self.sh("ls /lib/firmware/brcm 2>/dev/null | grep -ci apple; qlist -I | grep -cE 'apple-t2|tiny-dfr|t2fanrd'")
            self.step("non-mac-gets-no-t2-layer-and-no-apple-firmware", o.split() == ["0", "0"], o.strip().replace("\n", " "))

    def desktop_profile_checks(self, vm):
        u = self.user
        end = time.time() + qt.T(240)
        o = ""
        while time.time() < end:
            rc, o = self.sh("pgrep -x gdm >/dev/null && echo GDM; pgrep -x gnome-shell >/dev/null && echo SHELL")
            if "GDM" in o and "SHELL" in o:
                break
            time.sleep(6)
        self.step("desktop-gdm-greeter-running", "GDM" in o and "SHELL" in o, o.strip().replace("\n", " | "))
        qt.greeter_no_session_gate(self.sh, self.step, u, "gdm", wait=60)
        # a real PAM login of the profile's administrator with the password typed at install (login(1) on a pty)
        rc, o = self.sh(f"(sleep 2; printf '%s\\n' '{ZE.USER_PW}'; sleep 4; printf 'id -nG\\nexit\\n') | "
                        f"script -qec 'login {u}' /dev/null | tr -d '\\r' | tail -5", timeout=60)
        self.step("first-login-of-the-profile-administrator", "wheel" in o and "portage" in o, re.sub(r"\s+", " ", o)[:300])
        vm.shot("greeter")

    def declarative_change_checks(self):
        p = "/var/lib/horcrux/profile/bundle/profile.toml"
        rc, o = self.sh(f"cp {p} /root/profile.orig && python3 - <<'EOF'\nimport re\ns=open('{p}').read()\nif re.search(r'(?m)^packages = \\[', s):\n"
                        f"    s=re.sub(r'(?m)^packages = \\[', 'packages = [\"{EDIT_PKG}\", ', s, count=1)\nelse:\n    s += '\\n[software]\\npackages = [\"{EDIT_PKG}\"]\\n'\n"
                        f"open('{p}','w').write(s)\nEOF\nhorcrux-profile plan | grep '{EDIT_PKG}'; horcrux-profile apply > /root/apply2.log 2>&1; echo RC=$?; tail -3 /root/apply2.log; "
                        f"qlist -IC {EDIT_PKG}; grep -x {EDIT_PKG} /var/lib/portage/world; horcrux-profile generations | tail -2", timeout=1500)
        self.step("profile-edit-apply-adds-package-as-generation-2", "RC=0" in o and o.count(EDIT_PKG) >= 3 and re.search(r"(?m)^\s+2\s", o) is not None, re.sub(r"\s+", " ", o)[-400:])
        rc, o = self.sh(f"cp /root/profile.orig {p}; horcrux-profile apply --no-sync > /root/apply3.log 2>&1; echo RC=$?; grep -cx {EDIT_PKG} /var/lib/portage/world; "
                        "horcrux-profile generations | tail -1", timeout=900)
        self.step("profile-revert-deselects-package-as-generation-3", "RC=0" in o and re.search(r"(?m)^0$", o) is not None and re.search(r"(?m)^\s+3\s", o) is not None, re.sub(r"\s+", " ", o)[-300:])


def make_mac_esp(path):
    """a disk image with one GPT EFI system partition holding t2linux's firmware-raw.tar.gz made from a SYNTHETIC macOS firmware tree
    (same layout as /usr/share/firmware/wifi on a T2 Mac, made-up bytes: no Apple data). Needs sgdisk, mkfs.vfat, mcopy (mtools)."""
    import tarfile
    d = tempfile.mkdtemp()
    w = os.path.join(d, "wifi", "C-4364__s-B2")
    os.makedirs(w)
    for name, data in (("ekans.trx", b"\x01synthetic-ekans"), ("hanauma.trx", b"\x01synthetic-ekans"), ("ekans.clmb", b"synthetic-clm"),
                       ("P-ekans_M-HRPN_V-m__m-5.1.txt", b"boardrev=0x1\n"), ("P-ekans_M-HRPN_V-u__m-7.5.txt", b"boardrev=0x2\n")):
        open(os.path.join(w, name), "wb").write(data)
    tgz = os.path.join(d, "firmware-raw.tar.gz")
    with tarfile.open(tgz, "w:gz") as t:
        t.add(os.path.join(d, "wifi"), arcname="./wifi")
    subprocess.check_call(["truncate", "-s", "64M", path])
    subprocess.check_call(["sgdisk", "-o", "-n", "1:2048:0", "-t", "1:ef00", path], stdout=subprocess.DEVNULL)
    part = path + ".p1"
    subprocess.check_call(["truncate", "-s", str(64 * 1024 * 1024 - 2048 * 512 - 34 * 512), part])
    subprocess.check_call(["mkfs.vfat", "-n", "EFI", part], stdout=subprocess.DEVNULL)
    if shutil.which("mcopy"):
        subprocess.check_call(["mcopy", "-i", part, tgz, "::firmware-raw.tar.gz"])
    else:          # no mtools: loop-mount (needs root or passwordless sudo)
        m = tempfile.mkdtemp()
        pre = [] if os.geteuid() == 0 else ["sudo", "-n"]
        subprocess.check_call(pre + ["mount", "-o", "loop", part, m])
        try:
            subprocess.check_call(pre + ["cp", tgz, os.path.join(m, "firmware-raw.tar.gz")])
        finally:
            subprocess.check_call(pre + ["umount", m])
            os.rmdir(m)
    subprocess.check_call(["dd", f"if={part}", f"of={path}", "bs=512", "seek=2048", "conv=notrunc", "status=none"])
    os.unlink(part)
    shutil.rmtree(d)
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iso", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--profile", default="", help="bundle directory with profile.toml")
    ap.add_argument("--desktop", action="store_true")
    ap.add_argument("--fake-t2", action="store_true")
    ap.add_argument("--mac-esp", default="", help="raw disk image with a Mac EFI partition holding firmware-raw.tar.gz (synthetic)")
    ap.add_argument("--dev-overlay", default="", help="the branch's gentoo/ overlay, bundled as repo horcrux-dev (pre-merge runs)")
    ap.add_argument("--private-overlay", default="", help="private overlay checkout for profiles that name bundle:repos/ctg-private")
    ap.add_argument("--mem", type=int, default=6144)
    ap.add_argument("--cpu", default=os.environ.get("QEMU_TEST_CPU", "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"))
    ap.add_argument("--disk-gb", type=int, default=40)
    ap.add_argument("--keep-disks", action="store_true")
    ap.add_argument("--make-mac-esp", default="", help="only write the synthetic Mac ESP image to this path and exit")
    a = ap.parse_args()
    if a.make_mac_esp:
        print(make_mac_esp(a.make_mac_esp))
        return
    qt.ARCH = qt.HOST_ARCH
    qt.SLOW = float(os.environ.get("QEMU_SLOW", 0) or 0) or (1.0 if os.path.exists("/dev/kvm") else 3.0)
    a.mem, note = qt.fit_guest_mem(a.mem, qt.cgroup_mem_limit_mb())
    os.makedirs(a.out, exist_ok=True)
    a.bundle = os.path.join(a.out, "bundle")
    if os.path.exists(a.bundle):
        shutil.rmtree(a.bundle)
    mp = build_bundle(a.profile, a.bundle, a.dev_overlay or None, a.private_overlay or None)
    errs = E.problems(mp, a.bundle)
    if errs:
        raise SystemExit("profile invalid: " + "; ".join(errs))
    if a.fake_t2 and not a.mac_esp:
        a.mac_esp = make_mac_esp(os.path.join(a.out, "mac-esp.img"))
    try:
        PRun(a, mp).run()
    finally:
        if not a.keep_disks:
            for n in ("target.qcow2", "target2.qcow2", "answers.img", "OVMF_VARS.fd", "mac-esp.img"):
                try:
                    os.unlink(os.path.join(a.out, n))
                except OSError:
                    pass


if __name__ == "__main__":
    main()
