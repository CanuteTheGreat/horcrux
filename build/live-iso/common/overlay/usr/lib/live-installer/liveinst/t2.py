"""Apple T2 Mac support for the installer (generic: every Mac with the T2 security chip).

Detection is by PCI id 106b:1801 (the "T2 Bridge Controller" that apple-bce drives); no model list.
Nothing Apple-proprietary is shipped in the ISO: Wi-Fi/Bluetooth firmware is imported from an existing
install (/lib/firmware/brcm/*apple*) or from a t2linux `firmware.tar` at install time.

Pure functions where possible so they are unit-testable without hardware (tests/test_t2.py).
"""
import fnmatch
import glob
import os
import re
import shutil
import tarfile

T2_PCI_VENDOR = "0x106b"
T2_PCI_DEVICE = "0x1801"           # T2 Bridge Controller (apple-bce); 1802 = Secure Enclave, 1803 = audio

# Kernel modules that must be in the initramfs and loaded BEFORE the LUKS prompt: the internal keyboard and
# trackpad hang off the T2 and only exist once the apple-bce stack (t2bce_*: core, dma, vhci = virtual USB
# host controller) is up. hid_apple/appletb* give the function-key layer, usbhid/hid_generic the HID base.
INITRAMFS_DRIVERS = [
    "t2bce_core", "t2bce_dma", "t2bce_vhci",
    "hid", "hid_generic", "usbhid", "hid_apple", "hid_appletb_kbd", "hid_appletb_bl", "hid_magicmouse",
    "nvme", "dm_crypt", "dm_mod", "xhci_pci", "xhci_hcd",
]
# loaded by dracut at the very beginning of the initramfs (rd.driver.pre), in this order
EARLY_DRIVERS = ["t2bce_core", "t2bce_dma", "t2bce_vhci", "hid_apple", "hid_appletb_kbd"]
# the audio part (t2bce_audio) is not needed to boot; the full set is loaded by modules-load.d later
LATE_MODULES = ["t2bce_core", "t2bce_dma", "t2bce_vhci", "t2bce_audio", "hid_apple", "hid_appletb_kbd",
                "hid_appletb_bl", "brcmfmac", "brcmfmac_wcc", "hci_bcm4377", "apple_mfi_fastcharge"]

# suspend needs pm_async=off on T2 Macs (the T2 does not come back from an async resume reliably)
KERNEL_CMDLINE = ["pm_async=off", "intel_iommu=on", "iommu=pt"]

# the Wi-Fi/BT firmware names the t2linux firmware.sh produces
FW_PATTERNS = ["brcmfmac*", "brcmbt*", "*apple*", "BCM*.hcd", "*.clm_blob"]
FW_SUFFIXES = (".bin", ".txt", ".clm_blob", ".ptb", ".hcd", ".trx")
FW_MAX_FILE = 8 * 1024 * 1024
FW_MAX_TOTAL = 64 * 1024 * 1024
FW_MAX_FILES = 400


# --------------------------------------------------------------------------
# detection
# --------------------------------------------------------------------------
def _rd(path):
    try:
        with open(path) as f:
            return f.read().strip().lower()
    except OSError:
        return ""


def detect(sysfs_pci="/sys/bus/pci/devices", dmi="/sys/class/dmi/id"):
    """True on an Apple T2 Mac: PCI 106b:1801 present. The DMI vendor is only informational."""
    for d in glob.glob(os.path.join(sysfs_pci, "*")):
        if _rd(os.path.join(d, "vendor")) == T2_PCI_VENDOR and _rd(os.path.join(d, "device")) == T2_PCI_DEVICE:
            return True
    return False


def model(dmi="/sys/class/dmi/id"):
    return _rd(os.path.join(dmi, "product_name")) and open(os.path.join(dmi, "product_name")).read().strip()


# --------------------------------------------------------------------------
# generated configuration
# --------------------------------------------------------------------------
def dracut_conf(root="/"):
    # lvm only when the image has it (Surface-style LVM swap); `dracut` ABORTS on a module that cannot be installed
    lvm = " lvm" if os.path.exists(os.path.join(root, "usr/sbin/lvm")) or os.path.exists(os.path.join(root, "sbin/lvm")) else ""
    return ("# Horcrux T2 profile: the internal keyboard/trackpad of a T2 Mac is a USB device behind the T2 and\n"
            "# only appears once apple-bce (t2bce_*) is loaded. Without these lines the LUKS prompt is unusable.\n"
            'hostonly="no"\n'
            'add_drivers+=" ' + " ".join(INITRAMFS_DRIVERS) + ' "\n'
            'force_drivers+=" ' + " ".join(EARLY_DRIVERS) + ' "\n'
            'add_dracutmodules+=" crypt' + lvm + ' dm "\n'
            'omit_dracutmodules+=" plymouth "\n')


def modules_load_conf():
    return "# Horcrux T2 profile\n" + "\n".join(LATE_MODULES) + "\n"


def modprobe_conf():
    # apple-bce must come up before the HID drivers that bind to its virtual USB devices
    return ("# Horcrux T2 profile\n"
            "softdep hid_apple pre: t2bce_vhci\n"
            "softdep hid_appletb_kbd pre: t2bce_vhci\n"
            "softdep t2bce_vhci pre: t2bce_core t2bce_dma\n"
            "# keep the Touch Bar in function-key mode by default (1 = fn row), change with the Fn key\n"
            "options hid_appletb_kbd mode=1\n")


def grub_cmdline_extra():
    return " ".join(KERNEL_CMDLINE)


def initramfs_check(listing):
    """Verify an initramfs listing (lsinitrd output or a list of paths) contains the apple-bce stack + hid_apple.
    Module file names use '-' or '_' (hid-apple.ko, t2bce_core.ko): both are accepted. Returns the missing modules
    (empty = ok). This is a hard gate of the installer and of the T2 image build."""
    text = "\n".join(listing) if not isinstance(listing, str) else listing
    missing = []
    for mod in ("t2bce_core", "t2bce_vhci", "t2bce_dma", "hid_apple"):
        if not re.search(r"(?<![A-Za-z0-9])" + mod.replace("_", "[-_]") + r"\.ko(\.\w+)?\b", text):
            missing.append(mod)
    return missing


# --------------------------------------------------------------------------
# firmware import (never shipped in the ISO)
# --------------------------------------------------------------------------
def _wanted(name):
    b = os.path.basename(name)
    return b.endswith(FW_SUFFIXES) and any(fnmatch.fnmatch(b, p) for p in FW_PATTERNS) and ("/" not in b)


def _safe_member(m):
    """tar member we accept: a regular file, relative path without '..', wanted firmware name, sane size."""
    if not m.isreg():
        return False
    n = m.name
    if n.startswith("/") or ".." in n.split("/"):
        return False
    return _wanted(n) and 0 < m.size <= FW_MAX_FILE


def list_firmware_dir(d):
    """Apple-specific firmware files in an existing install's /lib/firmware/brcm (generic Broadcom blobs come from
    linux-firmware and are not imported)"""
    out = []
    if os.path.isdir(d):
        for n in sorted(os.listdir(d)):
            p = os.path.join(d, n)
            if os.path.isfile(p) and not os.path.islink(p) and _wanted(n) and "apple" in n.lower():
                out.append(p)
    return out


def apple_fw_count(files):
    return sum(1 for f in files if "apple" in os.path.basename(f).lower())


def list_firmware_tar(path):
    """accepted members of a firmware tarball (.tar/.tar.gz/.tar.xz/.tgz), as (name, size) tuples"""
    with tarfile.open(path) as t:
        return [(m.name, m.size) for m in t.getmembers() if _safe_member(m)]


def find_firmware_sources(roots=("/mnt", "/media", "/run/media", "/run/old-os"), extra=()):
    """Look for importable firmware: an existing install's brcm dir, or a firmware.tar*, below `roots`.
    Returns a list of dicts {kind: 'dir'|'tar', path, files, apple}. Cheap: bounded depth."""
    found = []
    seen = set()
    for r in list(roots) + list(extra):
        if not r or not os.path.exists(r):
            continue
        if os.path.isfile(r) and ".tar" in r or r.endswith(".tgz"):
            cands = [r]
        else:
            cands = []
            base = r.rstrip("/")
            depth0 = base.count("/")
            for dp, dn, fn in os.walk(base):
                if dp.count("/") - depth0 > 6:
                    dn[:] = []
                    continue
                dn[:] = [d for d in dn if d not in ("proc", "sys", "dev", ".cache", "node_modules", "target", "Nextcloud", ".rustup", ".cargo")]
                if dp.endswith("lib/firmware/brcm"):
                    cands.append(dp)
                for f in fn:
                    if re.fullmatch(r"firmware\.(tar|tar\.gz|tar\.xz|tgz)", f):
                        cands.append(os.path.join(dp, f))
        for c in cands:
            rc = os.path.realpath(c)
            if rc in seen:
                continue
            seen.add(rc)
            try:
                if os.path.isdir(c):
                    files = list_firmware_dir(c)
                    if apple_fw_count(files):
                        found.append({"kind": "dir", "path": c, "files": len(files), "apple": apple_fw_count(files)})
                else:
                    mem = list_firmware_tar(c)
                    ap = sum(1 for n, _s in mem if "apple" in os.path.basename(n).lower())
                    if ap:
                        found.append({"kind": "tar", "path": c, "files": len(mem), "apple": ap})
            except (OSError, tarfile.TarError):
                continue
    return found


def import_firmware(src, target_root, kind=None):
    """Copy firmware from `src` (directory or tarball) into <target_root>/lib/firmware/brcm.
    Rejects path tricks, symlinks, oversize or unrelated files. Returns the list of installed file names."""
    dest = os.path.join(target_root, "lib/firmware/brcm")
    os.makedirs(dest, exist_ok=True)
    done, total = [], 0
    if kind is None:
        kind = "dir" if os.path.isdir(src) else "tar"
    if kind == "dir":
        for p in list_firmware_dir(src):
            size = os.path.getsize(p)
            if size > FW_MAX_FILE or total + size > FW_MAX_TOTAL or len(done) >= FW_MAX_FILES:
                continue
            shutil.copyfile(p, os.path.join(dest, os.path.basename(p)))
            total += size
            done.append(os.path.basename(p))
    else:
        with tarfile.open(src) as t:
            for m in t.getmembers():
                if not _safe_member(m) or total + m.size > FW_MAX_TOTAL or len(done) >= FW_MAX_FILES:
                    continue
                f = t.extractfile(m)
                if f is None:
                    continue
                b = os.path.basename(m.name)
                with open(os.path.join(dest, b), "wb") as o:
                    shutil.copyfileobj(f, o)
                total += m.size
                done.append(b)
    for b in done:
        os.chmod(os.path.join(dest, b), 0o644)
    return sorted(set(done))


def iso_ships_apple_firmware(root="/"):
    """True if the tree under `root` contains Apple-specific brcm firmware (must be False for the public ISO)"""
    return any("apple" in n.lower() for n in glob.glob(os.path.join(root, "lib/firmware/brcm/*")))


# --------------------------------------------------------------------------
# applying the profile to an installed tree
# --------------------------------------------------------------------------
T2_ASSETS = "/usr/share/horcrux/t2"          # kernel tarball, tiny-dfr/t2fanrd init scripts, UCM files (in the image)


def target_kernel_version(target):
    """newest kernel in <target>/lib/modules whose name marks the T2 build (…-t2…) else None"""
    vs = sorted(os.listdir(os.path.join(target, "lib/modules"))) if os.path.isdir(os.path.join(target, "lib/modules")) else []
    t2 = [v for v in vs if "t2" in v]
    return t2[-1] if t2 else None


def install_audio(inst):
    """t2linux audio profile sets (PipeWire/PulseAudio card profiles + the udev rule that picks the AppleT2xN set)"""
    src = os.path.join(inst.target, T2_ASSETS.lstrip("/"), "audio")
    if not os.path.isdir(src):
        return False
    for mixer in ("usr/share/alsa-card-profile/mixer", "usr/share/pulseaudio/alsa-mixer"):
        for sub in ("profile-sets", "paths"):
            dst = os.path.join(inst.target, mixer, sub)
            os.makedirs(dst, exist_ok=True)
            for f in os.listdir(os.path.join(src, sub)):
                shutil.copy2(os.path.join(src, sub, f), os.path.join(dst, f))
    rules = os.path.join(inst.target, "usr/lib/udev/rules.d")
    os.makedirs(rules, exist_ok=True)
    shutil.copy2(os.path.join(src, "91-audio-custom.rules"), os.path.join(rules, "91-audio-custom.rules"))
    return True


def write_config(inst):
    """write the T2 dracut / modprobe / modules-load config into the installed tree (inst.write)"""
    inst.write("/etc/dracut.conf.d/90-horcrux-t2.conf", dracut_conf(inst.target))
    inst.write("/etc/modprobe.d/90-horcrux-t2.conf", modprobe_conf())
    inst.write("/etc/modules-load.d/horcrux-t2.conf", modules_load_conf())
    if install_audio(inst):
        inst.log("T2 audio: t2linux profile sets installed")
    # services (OpenRC); each only when the package/init script is in the image
    for svc in ("t2fanrd", "tiny-dfr"):
        if os.path.exists(f"{inst.target}/etc/init.d/{svc}") and os.path.exists(f"{inst.target}/usr/bin/{svc}"):
            inst.run(["rc-update", "add", svc, "default"], chroot=True, check=False)


def rebuild_initramfs(inst, kver):
    """dracut for the T2 kernel, then HARD CHECK that the apple-bce stack is inside (keyboard at the LUKS prompt)"""
    img = f"/boot/initramfs-{kver}.img"
    gcc = f"{inst.target}/usr/lib/gcc"       # no Portage tree in the target: dracut's zfs module asks gcc-config for the CHOST
    chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
    inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, img, kver], chroot=True, env={"CHOST": chost})
    out = []
    inst.run(["lsinitrd", img], chroot=True, line_cb=lambda l: out.append(l))
    miss = initramfs_check(out)
    if miss:
        from .system import InstallError
        raise InstallError("T2 initramfs is missing " + ", ".join(miss) + " - the keyboard would be dead at the LUKS prompt")
    inst.log(f"T2 initramfs {img} contains apple-bce ({', '.join(EARLY_DRIVERS)} loaded early)")
    return img


def install_kernel_from_assets(inst):
    """unpack a bundled T2 kernel tarball (kernel-*.tar.*: /boot/vmlinuz-<ver>-t2*, /lib/modules/<ver>-t2*) into the target"""
    for tb in sorted(glob.glob(os.path.join(T2_ASSETS, "kernel-*.tar*"))):
        inst.run(["tar", "-xpf", tb, "-C", inst.target, "--no-same-owner"])
        inst.run(["depmod", "-a", target_kernel_version(inst.target) or ""], chroot=True, check=False)
        return True
    return False


def firmware_choice(inst):
    """where to import the Apple Wi-Fi/BT firmware from: answer-file path, or the first source found by scanning.
    Returns (kind, path) or None (= no firmware available: the machine boots, Wi-Fi/Bluetooth stay off)."""
    want = str(inst.c.get("t2_firmware", "auto"))
    if want == "none":
        return None
    if want != "auto":
        return ("dir" if os.path.isdir(want) else "tar", want)
    roots = [r for r in ("/mnt", "/media", "/run/media", "/run/old-os", "/root", "/home") if r != inst.target]
    found = find_firmware_sources(roots)
    return (found[0]["kind"], found[0]["path"]) if found else None


def apply(inst):
    """T2 profile for the installed tree (called from Installer.profile_extras, after copy_system)"""
    from .system import InstallError
    inst.log("Apple T2 Mac detected (PCI 106b:1801): applying the T2 profile")
    kver = target_kernel_version(inst.target)
    if not kver and install_kernel_from_assets(inst):
        kver = target_kernel_version(inst.target)
    if not kver:
        msg = ("this image has no T2 kernel (apple-bce): the internal keyboard would not work at the disk-passphrase "
               "prompt. Use the Apple T2 image of Horcrux.")
        if inst.c.get("zfs_encrypt") or not inst.c.get("t2_allow_no_kernel"):
            raise InstallError(msg)
        inst.log("WARNING: " + msg)
        return
    write_config(inst)
    fw = firmware_choice(inst)
    if fw:
        done = import_firmware(fw[1], inst.target, fw[0])
        inst.log(f"Apple firmware: imported {len(done)} files from {fw[0]} {fw[1]}")
    else:
        inst.log("Apple firmware: none found - Wi-Fi/Bluetooth need `horcrux-t2-firmware <firmware.tar|dir>` after the first boot")
    rebuild_initramfs(inst, kver)
    inst.cmdline_extra = (inst.cmdline_extra + " " + grub_cmdline_extra()).strip()
    inst.kernel_glob = "*t2*"


def image_has_t2_kernel(root="/"):
    """the running image carries a T2 kernel (…-t2… in /lib/modules) or a bundled kernel tarball"""
    return bool(target_kernel_version(root)) or bool(glob.glob(os.path.join(root, T2_ASSETS.lstrip("/"), "kernel-*.tar*")))


def preflight(inst):
    """called before the disk is touched: never erase a disk when the result could not boot with its keyboard"""
    from .system import InstallError
    if not image_has_t2_kernel("/") and (inst.c.get("zfs_encrypt") or not inst.c.get("t2_allow_no_kernel")):
        raise InstallError("this image has no T2 kernel (apple-bce): the internal keyboard would not work at the disk-passphrase "
                           "prompt. Use the Apple T2 image of Horcrux. (nothing was written to the disk)")
