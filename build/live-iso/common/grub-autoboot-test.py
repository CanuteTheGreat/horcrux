#!/usr/bin/env python3
"""Headless-boot gate for the boot-environment GRUB menu (horcrux-be grubcfg / liveinst.be.render_grub).

A machine must NEVER sit at the boot menu waiting for a key. GRUB turns its countdown off for good once an automatic boot attempt
comes back to the menu (grub_menu_execute_entry unsets `timeout`), so a kernel/initrd that fails to load once leaves the machine at
a menu with no timer. This gate boots the generated grub.cfg under UEFI in QEMU, never sends a single key, and asserts that a
kernel starts (its "Kernel command line:" reaches the serial console) within timeout + 10 s of the menu (+ 10 s to load and
decompress the kernel, + GRUB's 10 s error pause per expected failed attempt), and that the RIGHT kernel started:

  normal          empty environment block                          -> default kernel
  no-env          no grubenv file at all                           -> default kernel, no error pause
  one-shot        next_entry = previous kernel                     -> previous kernel once; next_entry cleared; next boot -> default
  readonly-env    next_entry set, disk read-only (save_env fails)  -> default kernel (a trial that cannot be cleared is never booted)
  broken-default  default kernel unloadable                        -> previous kernel through `fallback`
  trial-broken    newest kernel on trial (next_entry) unloadable   -> known-good kernel
  all-broken      every kernel unloadable                          -> GRUB retries, then restarts the machine (never waits)
  control-no-fallback  broken default with `fallback` removed     -> must HANG (proves the gate sees a menu waiting for a key)

It uses the IMAGE's own GRUB (modules + grub-mkimage from the ISO's squashfs, run with the image's own dynamic loader) and the
image's kernel, so each image (OpenRC, systemd, each arch) gates its own boot loader. Needs: qemu, OVMF/AAVMF, xorriso,
unsquashfs, mke2fs (-d), mtools + mkfs.vfat, sgdisk, debugfs. No root, no loop devices.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import time
import uuid as uuidlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "overlay/usr/lib/live-installer"))
from liveinst import be as BE  # noqa: E402  (the same program the image installs as horcrux-be)

ARCHES = {
    "x86_64": dict(target="x86_64-efi", efi="BOOTX64.EFI", qemu="qemu-system-x86_64", console="ttyS0,115200",
                   serial="serial --unit=0 --speed=115200",
                   fw=[("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
                       ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd")]),
    "aarch64": dict(target="arm64-efi", efi="BOOTAA64.EFI", qemu="qemu-system-aarch64", console="ttyAMA0,115200", serial=None,
                    fw=[("/usr/share/AAVMF/AAVMF_CODE.fd", "/usr/share/AAVMF/AAVMF_VARS.fd"),
                        ("/usr/share/AAVMF/AAVMF_CODE.no-secboot.fd", "/usr/share/AAVMF/AAVMF_VARS.fd"),
                        ("/usr/share/edk2/aarch64/QEMU_EFI-pflash.raw", "/usr/share/edk2/aarch64/vars-template-pflash.raw")]),
}
MODS = "part_gpt ext2 fat normal linux search search_fs_uuid configfile echo test loadenv sleep reboot gzio serial terminal efi_gop all_video"
PREV = "0.0.1-previous"   # kernel version label of the "previous kernel" entry (same kernel binary, other path)
TIMEOUT = 5
ERR_PAUSE = 10            # GRUB's "Press any key to continue..." after a failed entry (grub_wait_after_message, 10 s)
LOAD = 10                 # reading ~20 MB of kernel through GRUB's EFI disk driver + decompression up to the first console line (KVM)
RESULTS = []


def log(msg):
    print(msg, flush=True)


def step(name, ok, detail=""):
    RESULTS.append((name, ok))
    log(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f": {detail}" if detail else ""))


def run(cmd, **kw):
    kw.setdefault("check", True)
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


class Image:
    """the image's GRUB modules, its grub-mkimage (run through the image's own loader), its kernel"""

    def __init__(self, iso, work, arch):
        self.a = ARCHES[arch]
        self.work = work
        sq = os.path.join(work, "squashfs.img")
        run(["xorriso", "-osirrox", "on", "-indev", iso, "-extract", "/LiveOS/squashfs.img", sq])
        run(["xorriso", "-osirrox", "on", "-indev", iso, "-extract", "/boot/vmlinuz", os.path.join(work, "vmlinuz")])
        self.root = os.path.join(work, "root")
        # only what grub-mkimage needs: its modules, the binary, the image's dynamic loader and the libraries it links (by NEEDED)
        listing = run(["unsquashfs", "-lc", "-d", "", sq], check=False).stdout.split()
        def pick(names):
            return [p.lstrip("/") for p in listing if os.path.basename(p) in names and re.match(r"/?(usr/)?lib(64)?/[^/]+$", p)]
        def extract(paths):
            if paths:
                run(["unsquashfs", "-no-progress", "-q", "-f", "-follow-symlinks", "-d", self.root, sq] + paths, check=False)
        extract([f"usr/lib/grub/{self.a['target']}", "usr/bin/grub-mkimage"])
        want, seen = {"ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"}, set()
        todo = [os.path.join(self.root, "usr/bin/grub-mkimage")]
        while todo:
            f = todo.pop()
            out = run(["readelf", "-d", f], check=False).stdout if os.path.isfile(f) else ""
            libs = set(re.findall(r"Shared library: \[([^\]]+)\]", out)) - seen
            seen |= libs
            want |= libs
            paths = pick(libs | want)
            extract(paths)
            todo += [os.path.join(self.root, p) for p in paths if os.path.basename(p) in libs]
        os.unlink(sq)
        self.mods = os.path.join(self.root, f"usr/lib/grub/{self.a['target']}")
        if not os.path.isfile(os.path.join(self.mods, "normal.mod")):
            raise SystemExit(f"image has no GRUB {self.a['target']} modules")
        self.kernel = os.path.join(work, "vmlinuz")

    def mkimage(self, out, early):
        ldname = "ld-linux-x86-64.so.2" if self.a["target"] == "x86_64-efi" else "ld-linux-aarch64.so.1"
        lds = [p for p in (os.path.join(self.root, d, ldname) for d in ("lib64", "lib", "usr/lib64", "usr/lib")) if os.path.exists(p)]
        libp = ":".join(os.path.join(self.root, d) for d in ("lib64", "usr/lib64", "lib", "usr/lib"))
        tool = [lds[0], "--library-path", libp, os.path.join(self.root, "usr/bin/grub-mkimage")] if lds else ["grub-mkimage"]
        mods = [m for m in MODS.split() if os.path.isfile(os.path.join(self.mods, m + ".mod"))]
        run(tool + ["-O", self.a["target"], "-d", self.mods, "-o", out, "-p", "/grub", "-c", early] + mods)


def grubenv(path, **vars_):
    """GRUB environment block (what grub-editenv writes): 1024 bytes, header, var=value lines, '#' padding"""
    body = "# GRUB Environment Block\n# WARNING: Do not edit this file by tools other than grub-editenv!!!\n"
    body += "".join(f"{k}={v}\n" for k, v in vars_.items())
    open(path, "w").write(body + "#" * (1024 - len(body)))


def build_disk(img, case, d):
    """GPT disk: ESP (the GRUB EFI image, finds /boot by UUID like `grub-install --removable`) + ext4 /boot laid out like an
    installed system (/be/<be>/vmlinuz, /be/<be>/kernels/<kver>/, /grub/grub.cfg, /grub/grubenv, /grub/<target>/*.mod)"""
    w = os.path.join(d, "tree")
    shutil.rmtree(w, ignore_errors=True)
    for p in ("be/horcrux/kernels/" + PREV, "grub"):
        os.makedirs(os.path.join(w, p))
    good = open(img.kernel, "rb").read()
    bad = b"not a kernel " * 4096
    newest, prev = good, good
    if case["broken"] in ("default", "all"):
        newest = bad
    if case["broken"] == "all":
        prev = bad
    open(os.path.join(w, "be/horcrux/vmlinuz"), "wb").write(newest)
    open(os.path.join(w, "be/horcrux/kernels", PREV, "vmlinuz"), "wb").write(prev)
    for p in ("be/horcrux/initramfs.img", f"be/horcrux/kernels/{PREV}/initramfs.img"):
        open(os.path.join(w, p), "wb").write(b"07070100000000000000000000000000000000000000000000000000000000000000000000000000000000000b00000000TRAILER!!!\0")
    shutil.copytree(img.mods, os.path.join(w, "grub", img.a["target"]))
    uuid = str(uuidlib.uuid4())
    cfg = BE.render_grub(["horcrux"], "horcrux", "rpool", uuid, f"console={img.a['console']} panic=0 loglevel=7", TIMEOUT,
                         img.a["serial"], kernels={"horcrux": "0.0.2-newest"}, previous={"horcrux": [PREV]},
                         default=case.get("default"), good={"horcrux": case.get("good", "0.0.2-newest")})
    if case.get("strip"):
        cfg = "".join(l for l in cfg.splitlines(True) if not l.lstrip().startswith("set fallback="))
    open(os.path.join(w, "grub/grub.cfg"), "w").write(cfg)
    if case.get("env") is not None:
        grubenv(os.path.join(w, "grub/grubenv"), **case["env"])
    boot = os.path.join(d, "boot.img")
    for f in (boot, os.path.join(d, "esp.img"), os.path.join(d, "disk.img")):
        if os.path.exists(f):
            os.unlink(f)
    run(["mke2fs", "-q", "-F", "-t", "ext4", "-U", uuid, "-L", "boot", "-d", w, boot, "192M"])
    early = os.path.join(d, "early.cfg")
    open(early, "w").write(f"search --no-floppy --fs-uuid --set=root {uuid}\nset prefix=($root)/grub\n")
    efi = os.path.join(d, img.a["efi"])
    img.mkimage(efi, early)
    esp = os.path.join(d, "esp.img")
    run(["mkfs.vfat", "-C", esp, str(32 * 1024)])
    run(["mmd", "-i", esp, "::EFI", "::EFI/BOOT"])
    run(["mcopy", "-i", esp, efi, f"::EFI/BOOT/{img.a['efi']}"])
    disk = os.path.join(d, "disk.img")
    with open(disk, "wb") as f:
        f.truncate(240 * 1024 * 1024)
    run(["sgdisk", "-n", "1:2048:+32M", "-t", "1:ef00", "-n", "2:0:+192M", "-t", "2:8300", disk])
    starts = [int(re.search(r"First sector: (\d+)", run(["sgdisk", "-i", str(n), disk]).stdout).group(1)) for n in (1, 2)]
    for part, start in zip((esp, boot), starts):
        run(["dd", f"if={part}", f"of={disk}", "bs=512", f"seek={start}", "conv=notrunc", "status=none"])
    os.unlink(esp)
    os.unlink(boot)
    return disk, starts[1]


def read_env(disk, start, d):
    """the grubenv GRUB left on the disk"""
    part = os.path.join(d, "boot-after.img")
    run(["dd", f"if={disk}", f"of={part}", "bs=512", f"skip={start}", f"count={192 * 2048}", "status=none"])
    out = run(["debugfs", "-R", "cat /grub/grubenv", part], check=False).stdout
    os.unlink(part)
    return dict(l.split("=", 1) for l in out.splitlines() if "=" in l and not l.startswith("#"))


def boot(img, disk, d, name, readonly=False, limit=240):
    """boot once with NO input at all; returns (serial text, seconds from menu to kernel or None, qemu exited by itself)"""
    a = img.a
    code, vars_ = next(((c, v) for c, v in a["fw"] if os.path.exists(c) and os.path.exists(v)), (None, None))
    if not code:
        raise SystemExit("no UEFI firmware (apt: ovmf / qemu-efi-aarch64)")
    nv = os.path.join(d, "vars.fd")
    shutil.copy(vars_, nv)
    if os.path.getsize(nv) < os.path.getsize(code):
        with open(nv, "ab") as f:
            f.truncate(os.path.getsize(code))
    kvm = os.access("/dev/kvm", os.W_OK) and os.uname().machine in (("x86_64",) if a["target"] == "x86_64-efi" else ("aarch64",))
    if a["target"] == "x86_64-efi":
        mach = ["-machine", "q35" + (",accel=kvm" if kvm else ""), "-cpu", "max" if not kvm else "host"]
    else:
        mach = ["-machine", "virt", "-cpu", "host" if kvm else "cortex-a72"] + (["-accel", "kvm"] if kvm else [])
    ser = os.path.join(d, f"serial-{name}.log")
    cmd = [a["qemu"], *mach, "-m", "1024", "-nodefaults", "-display", "none", "-no-reboot",
           "-drive", f"if=pflash,format=raw,readonly=on,file={code}", "-drive", f"if=pflash,format=raw,file={nv}",
           "-drive", f"if=none,id=d0,format=raw,file={disk}" + (",readonly=on" if readonly else ""),
           "-device", "virtio-blk-pci,drive=d0", "-serial", f"file:{ser}", "-monitor", "none"]
    if a["target"] == "x86_64-efi":
        cmd += ["-vga", "std"]   # a real console next to the serial one, like a laptop (GRUB writes the menu to both)
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=open(os.path.join(d, f"qemu-{name}.err"), "w"))
    t0 = time.time()
    t_menu = t_kernel = None
    txt = ""
    exited = False
    while time.time() - t0 < limit:
        time.sleep(0.25)
        txt = open(ser, errors="replace").read() if os.path.exists(ser) else ""
        if t_menu is None and "Horcrux - horcrux" in txt:
            t_menu = time.time()
        if "Kernel command line:" in txt:
            t_kernel = time.time()
            time.sleep(1)
            txt = open(ser, errors="replace").read()
            break
        if p.poll() is not None:
            exited = True
            break
    if p.poll() is None:
        p.kill()
        p.wait()
    took = round(t_kernel - t_menu, 1) if t_kernel and t_menu else None
    return txt, took, exited, (round(time.time() - t_menu, 1) if t_menu else None)


def booted(txt):
    m = re.search(r"Kernel command line:.*?BOOT_IMAGE=(\S+)", txt)
    return m.group(1) if m else None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--iso", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--arch", default=os.environ.get("ARCH", os.uname().machine.replace("arm64", "aarch64")))
    ap.add_argument("--only", default="", help="comma list of cases")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    a.out = os.path.abspath(a.out)
    img = Image(os.path.abspath(a.iso), a.out, a.arch)
    kvm = os.access("/dev/kvm", os.W_OK)
    slack = 0 if kvm else 60      # TCG: the kernel needs far longer from "loaded" to its first console line
    DEF, PRV = "/be/horcrux/vmlinuz", f"/be/horcrux/kernels/{PREV}/vmlinuz"
    PRV_ID = BE.entry_id("horcrux", PREV)
    cases = [
        dict(name="normal", env={}, broken="", want=DEF, fails=0),
        dict(name="no-env", env=None, broken="", want=DEF, fails=0),
        dict(name="one-shot", env={"next_entry": PRV_ID}, broken="", want=PRV, fails=0, then=DEF),
        dict(name="readonly-env", env={"next_entry": PRV_ID}, broken="", want=DEF, fails=0, readonly=True),
        dict(name="broken-default", env={}, broken="default", want=PRV, fails=1),
        dict(name="trial-broken", env={"next_entry": "be-horcrux"}, broken="default", want=PRV, fails=1, default=PRV_ID, good=PREV),
        dict(name="all-broken", env={}, broken="all", want=None, fails=6),
        # control: the same broken default WITHOUT `fallback` (the config that left a machine at a menu with no countdown) must hang;
        # proves this gate detects a menu waiting for a key
        dict(name="control-no-fallback", env={}, broken="default", want="STUCK", fails=1, strip=True),
    ]
    only = [c for c in a.only.split(",") if c]
    for c in cases:
        if only and c["name"] not in only:
            continue
        d = os.path.join(a.out, c["name"])
        os.makedirs(d, exist_ok=True)
        disk, start = build_disk(img, c, d)
        budget = TIMEOUT + 10 + LOAD + ERR_PAUSE * c["fails"] + slack
        if c["want"] == "STUCK":
            txt, took, exited, since = boot(img, disk, d, "1", limit=budget + 60)
            step(f"{c['name']}: control - without fallback a failed default leaves GRUB at a menu with no countdown",
                 took is None and not exited and "Press any key to continue" in txt and txt.count("Horcrux - horcrux") >= 2,
                 f"kernel={took}, exited={exited}, menu drawn {txt.count('Horcrux - horcrux')}x")
        elif c["want"] is None:
            # every kernel fails: RETRY_ROUNDS rounds of (countdown + one pause per kernel), then GRUB restarts the machine (-no-reboot: QEMU exits)
            budget = (BE.RETRY_ROUNDS + 1) * (TIMEOUT + 3 * ERR_PAUSE) + 30 + slack
            txt, took, exited, since = boot(img, disk, d, "1", limit=budget + 120)
            ok = exited and "restarting the machine" in txt and since is not None and since <= budget
            step(f"{c['name']}: no kernel can start -> GRUB retries, then restarts the machine on its own", ok,
                 f"qemu exited={exited}, {since} s after the menu (budget {budget} s)")
        else:
            txt, took, exited, _ = boot(img, disk, d, "1", readonly=c.get("readonly", False), limit=budget + 180)
            got = booted(txt)
            step(f"{c['name']}: a kernel starts with no key pressed within {budget} s of the menu", took is not None and took <= budget,
                 f"menu->kernel {took} s")
            step(f"{c['name']}: the expected kernel started", got == c["want"], f"BOOT_IMAGE={got}, want {c['want']}")
            if c["fails"] == 0:
                step(f"{c['name']}: no error pause before the kernel", "Press any key to continue" not in txt)
            if "then" in c:
                env = read_env(disk, start, d)
                step(f"{c['name']}: GRUB cleared next_entry before booting the trial", not env.get("next_entry"), f"grubenv {env}")
                txt2, took2, _, _ = boot(img, disk, d, "2", limit=budget + 180)
                step(f"{c['name']}: the next boot is the default again (one-shot)", booted(txt2) == c["then"] and took2 is not None and took2 <= budget,
                     f"BOOT_IMAGE={booted(txt2)}, {took2} s")
        os.unlink(disk)
        shutil.rmtree(os.path.join(d, "tree"), ignore_errors=True)
    shutil.rmtree(img.root, ignore_errors=True)
    bad = [n for n, ok in RESULTS if not ok]
    log(f"{len(RESULTS) - len(bad)}/{len(RESULTS)} checks passed" + (f"; FAILED STEPS: {', '.join(bad)}" if bad else ""))
    return 1 if bad or not RESULTS else 0


if __name__ == "__main__":
    sys.exit(main())
