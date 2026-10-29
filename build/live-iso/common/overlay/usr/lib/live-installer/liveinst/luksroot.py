"""XFS root inside a LUKS2 container ("xfs-luks"): the default for machines with little RAM.

Layout (GPT, one disk):  1 = ESP (FAT32, 1 GiB; BIOS-boot 1 MiB on legacy BIOS)   unencrypted
                         2 = /boot (ext4, 1 GiB)                                  unencrypted (kernel + initramfs)
                         3 = LUKS2 -> /dev/mapper/hxroot -> XFS                   everything else
No swap partition (zram only, see the hardware profile), no LVM.

Everything that is pure (sizes, command lines, kernel arguments, dracut/fstab text) is a plain function so the
unit tests can check it without a disk; the Installer calls the thin `*_stage` helpers at its stage hooks.
"""
import os
import re
import time

FS_ID = "xfs-luks"
MAPPER = "hxroot"
# Everything the initramfs needs to ASK FOR AND ACCEPT a passphrase on a machine whose only keyboard is a USB/HID
# device (Surface Type Cover, T2 Mac keyboards ...): HID core + USB host controllers + multitouch/microsoft keyboards.
INITRAMFS_DRIVERS = (
    "usbhid hid_generic hid hid_multitouch hid_microsoft hid_apple hid_cherry hid_logitech_dj "
    "xhci_pci xhci_hcd ehci_pci ehci_hcd uhci_hcd ohci_pci "
    "atkbd i8042 i915 dm_crypt xfs ext4 vfat nls_cp437 nls_iso8859_1 aesni_intel sha256_ssse3 "
    "nvme ahci sd_mod mmc_block sdhci_pci virtio_blk virtio_pci"
)
ARGON_MEM_CAP_KIB = 1024 * 1024          # 1 GiB: cryptsetup's own maximum, and what 4 GB machines can spare
ARGON_MEM_MIN_KIB = 64 * 1024


def is_luks(cfg):
    return cfg.get("fs") == FS_ID


def pbkdf_memory_kib(ram_mb):
    """argon2id memory cost for the LUKS2 keyslot, in KiB: 3/16 of RAM, at least 64 MiB, at most 1 GiB.

    The unlock runs in the initramfs and needs that much FREE memory; a cost near total RAM stalls or OOM-kills the
    boot, and that failure is unrecoverable without another machine. 4 GB -> 768 MiB, 8 GB and up -> 1 GiB."""
    if ram_mb <= 0:
        return 262144
    kib = int(ram_mb * 1024 * 0.1875)
    return max(ARGON_MEM_MIN_KIB, min(ARGON_MEM_CAP_KIB, kib // 1024 * 1024))


def layout(disk, part_path, efi=True):
    return {1: part_path(disk, 1), 2: part_path(disk, 2), 3: part_path(disk, 3)}


def sgdisk_args(disk, efi):
    first = ["-n", "1:0:+1G", "-t", "1:ef00", "-c", "1:ESP"] if efi else ["-n", "1:0:+1M", "-t", "1:ef02", "-c", "1:BIOS-boot"]
    return ["sgdisk"] + first + ["-n", "2:0:+1G", "-t", "2:8300", "-c", "2:boot",
                                 "-n", "3:0:0", "-t", "3:8309", "-c", "3:cryptroot", disk]


def luks_format_cmd(dev, ram_mb):
    return ["cryptsetup", "luksFormat", "--type", "luks2", "--batch-mode", "--cipher", "aes-xts-plain64", "--key-size", "512",
            "--hash", "sha256", "--pbkdf", "argon2id", "--pbkdf-memory", str(pbkdf_memory_kib(ram_mb)), "--pbkdf-parallel", "2",
            "--iter-time", "2000", "--key-file=-", dev]


def luks_open_cmd(dev):
    # allow-discards: needed for the weekly fstrim to reach the SSD through dm-crypt (documented trade-off: it reveals
    # which blocks are unused). The mount itself never uses the `discard` option.
    return ["cryptsetup", "open", "--allow-discards", "--key-file=-", dev, MAPPER]


def kernel_args(luks_uuid, extra=""):
    a = [f"rd.luks.uuid={luks_uuid}", f"rd.luks.name={luks_uuid}={MAPPER}", f"rd.luks.allow-discards={luks_uuid}",
         f"root=/dev/mapper/{MAPPER}", "rootfstype=xfs"]
    return " ".join(a + ([extra] if extra else []))


def filter_drivers(names, modules_dep, modules_builtin=""):
    """keep only the drivers that exist as loadable modules of this kernel: dracut-install ABORTS the whole initramfs on one
    missing name (sha256_ssse3 does not exist as a module on the Gentoo dist kernel), and built-in ones need no listing."""
    have = set()
    for line in modules_dep.splitlines():
        m = re.match(r"(?:.*/)?([^/:]+?)\.ko(?:\.\w+)?:", line)
        if m:
            have.add(m.group(1).replace("-", "_"))
    return [n for n in names.split() if n.replace("-", "_") in have]


def dracut_conf(drivers=None):
    drivers = INITRAMFS_DRIVERS if drivers is None else drivers
    return ("# written by live-installer (xfs-luks): the initramfs must unlock the root with a USB/HID keyboard\n"
            'hostonly="no"\nhostonly_cmdline="no"\n'
            'add_dracutmodules+=" crypt dm "\n'
            'omit_dracutmodules+=" plymouth network-manager nfs iscsi biosdevname dmsquash-live "\n'
            f'add_drivers+=" {drivers} "\n')


def fstab_text(root_uuid_dev, boot_uuid, esp_uuid, efi=True):
    lines = ["# written by live-installer (xfs-luks)",
             f"{root_uuid_dev}\t/\txfs\tdefaults\t0 1",          # no `discard`: fstrim runs weekly instead
             f"UUID={boot_uuid}\t/boot\text4\tdefaults\t0 2"]
    if efi and esp_uuid:
        lines.append(f"UUID={esp_uuid}\t/boot/efi\tvfat\tumask=0077\t0 2")
    lines.append("tmpfs\t/tmp\ttmpfs\tdefaults,nosuid,nodev\t0 0")
    return "\n".join(lines) + "\n"


def crypttab_text(luks_uuid):
    return f"# the initramfs unlocks the root (rd.luks.*); this file is for tools\n{MAPPER}\tUUID={luks_uuid}\tnone\tluks,discard\n"


FSTRIM_CRON = """#!/bin/sh
# weekly TRIM (xfs-luks installs): the file system is mounted WITHOUT the discard option
/sbin/fstrim -av >/var/log/fstrim.log 2>&1 || true
"""


def validate(cfg, ram_mb=0):
    """problems with the LUKS part of a config (empty = ok)"""
    errs = []
    if not is_luks(cfg):
        return errs
    pw = cfg.get("luks_passphrase", "")
    if len(pw) < 12:
        errs.append("LUKS passphrase: at least 12 characters (the disk encryption is only as strong as this)")
    if len(cfg.get("disks", [])) > 1:
        errs.append("xfs-luks uses one disk")
    return errs


# ---------------------------------------------------------------- installer stage helpers (impure)
def partition_stage(inst):
    for disk in inst.c["disks"]:
        inst.run(["sh", "-c", f"swapoff -a 2>/dev/null; wipefs -a '{disk}' >/dev/null 2>&1; sgdisk -Z '{disk}' >/dev/null 2>&1; true"], check=False)
        inst.run(sgdisk_args(disk, inst.efi))
        inst.run(["partprobe", disk], check=False)
    inst.run("udevadm settle --timeout=20 || true", check=False)
    time.sleep(1)


def mkfs_stage(inst, ram_mb):
    L = inst.layout(inst.c["disks"][0])
    pw = inst.c["luks_passphrase"]
    if inst.efi:
        inst.run(["mkfs.vfat", "-F32", "-n", "ESP", L[1]])
    inst.run(["mkfs.ext4", "-F", "-q", "-L", "boot", L[2]])
    inst.log(f"LUKS2 argon2id, memory cost {pbkdf_memory_kib(ram_mb) // 1024} MiB (RAM {ram_mb} MB)")
    inst.run(luks_format_cmd(L[3], ram_mb), input_=pw)
    inst.run(luks_open_cmd(L[3]), input_=pw)
    inst.run(["mkfs.xfs", "-f", "-L", "root", f"/dev/mapper/{MAPPER}"])


def mount_stage(inst):
    L = inst.layout(inst.c["disks"][0])
    os.makedirs(inst.target, exist_ok=True)
    inst.run(["mount", f"/dev/mapper/{MAPPER}", inst.target])
    os.makedirs(inst.target + "/boot", exist_ok=True)
    inst.run(["mount", L[2], inst.target + "/boot"])
    if inst.efi:
        os.makedirs(inst.target + "/boot/efi", exist_ok=True)
        inst.run(["mount", L[1], inst.target + "/boot/efi"])


def blkid(dev, tag="UUID"):
    import subprocess
    r = subprocess.run(["blkid", "-s", tag, "-o", "value", dev], capture_output=True, text=True)
    return r.stdout.strip() if r.returncode == 0 else ""


def configure_stage(inst):
    """fstab, crypttab, dracut config, fstrim, then rebuild the initramfs INSIDE the target so it carries crypt + HID drivers"""
    L = inst.layout(inst.c["disks"][0])
    luks_uuid = blkid(L[3])
    if not luks_uuid:
        raise RuntimeError("cannot read the LUKS UUID of " + L[3])
    inst.luks_uuid = luks_uuid
    root_uuid = blkid(f"/dev/mapper/{MAPPER}")
    inst.write("/etc/fstab", fstab_text(f"UUID={root_uuid}" if root_uuid else f"/dev/mapper/{MAPPER}", blkid(L[2]), blkid(L[1]) if inst.efi else "", inst.efi))
    inst.write("/etc/crypttab", crypttab_text(luks_uuid))
    kvers = sorted(os.listdir(f"{inst.target}/lib/modules"))
    kver = kvers[-1]
    try:
        dep = open(f"{inst.target}/lib/modules/{kver}/modules.dep").read()
    except OSError:
        dep = ""
    avail = filter_drivers(INITRAMFS_DRIVERS, dep) if dep else []
    inst.log("initramfs drivers forced in: " + " ".join(avail) + " (the rest is built into the kernel or absent)")
    inst.write("/etc/dracut.conf.d/90-xfs-luks.conf", dracut_conf(" ".join(avail)))
    inst.write("/etc/cron.weekly/fstrim", FSTRIM_CRON, 0o755)
    inst.run(["rc-update", "add", "cronie", "default"], chroot=True, check=False)
    inst.log(f"rebuilding the initramfs for {kver} with crypt + USB/HID keyboard drivers")
    # dracut asks gcc-config for libgcc_s, which asks Portage for CHOST: the ISO has no Portage tree, so hand it over
    gcc = f"{inst.target}/usr/lib/gcc"
    chost = sorted(os.listdir(gcc))[0] if os.path.isdir(gcc) and os.listdir(gcc) else "x86_64-pc-linux-gnu"
    inst.run(["dracut", "--force", "--no-hostonly", "--kver", kver, f"/boot/initramfs-{kver}.img", kver], chroot=True, env={"CHOST": chost})
    import subprocess
    r = subprocess.run(["chroot", inst.target, "lsinitrd", f"/boot/initramfs-{kver}.img"], capture_output=True, text=True)
    cfgp = f"{inst.target}/boot/config-{kver}"
    cfg = open(cfgp).read() if os.path.exists(cfgp) else ""
    missing = verify_initramfs(r.stdout, cfg)
    inst.log("initramfs check (cryptsetup, dm-crypt, usbhid, xhci, hid-multitouch, hid-microsoft): " + ("all reachable at the passphrase prompt" if not missing else "MISSING " + ", ".join(missing)))
    if missing:
        raise RuntimeError("the new initramfs lacks " + ", ".join(missing) + ": the disk could not be unlocked at boot")


def verify_initramfs(img_listing, kernel_config=""):
    """static check on `lsinitrd` output (+ the kernel's /boot/config-*): the keyboard stack and cryptsetup are reachable at the
    passphrase prompt. A driver is fine when its .ko is in the initramfs OR the kernel has it built in (=y; the Gentoo dist kernel
    builds usbhid/xhci-pci/xfs in). Returns the missing item names."""
    need = {"cryptsetup": (r"cryptsetup", None), "dm-crypt": (r"dm-crypt", "CONFIG_DM_CRYPT"),
            "usbhid": (r"usbhid", "CONFIG_USB_HID"), "xhci": (r"xhci-(pci|hcd)", "CONFIG_USB_XHCI_PCI"),
            "hid-multitouch": (r"hid-multitouch", None), "hid-microsoft": (r"hid-microsoft", None)}
    missing = []
    for k, (rx, cfg) in need.items():
        if re.search(rx, img_listing) or (cfg and re.search(rf"^{cfg}=y$", kernel_config, re.M)):
            continue
        missing.append(k)
    return missing


def grub_cmdline_stage(inst):
    return kernel_args(inst.luks_uuid)


def finish_stage(inst):
    inst.run(f"cryptsetup close {MAPPER} 2>/dev/null; true", check=False)
