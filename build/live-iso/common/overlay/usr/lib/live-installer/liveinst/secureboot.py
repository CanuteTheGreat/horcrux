"""UEFI Secure Boot for installed systems (free: our own key + Microsoft-signed shim + MokManager).

What the image carries (boot/secureboot-sign.sh at ISO build time, in /usr/share/live-installer/secureboot/):
  shim<arch>.efi  Microsoft-signed shim (sys-boot/shim)       mm<arch>.efi  MokManager
  grub<arch>.efi  our GRUB, signed with the project key, reads the grub.cfg next to itself on the ESP
  ENROLL_THIS_KEY_IN_MOKMANAGER.cer  the project certificate (DER)
The kernels and out-of-tree modules of the image are already signed with the project key, so a fresh install boots with
Secure Boot on as soon as the project certificate is enrolled (the user did that once to boot the installer).

For updates the installer creates a MACHINE key (or takes the user's own key: bring your own key), configures Portage to
sign new kernels (secureboot.eclass) and kernel modules (linux-mod-r1 modules-sign) with it, and queues its certificate
for enrollment with mokutil: the next boot shows MokManager once, the user types the one-time password from the final
installer page. Until then the system keeps booting the project-signed kernel.

Config keys (answer file section [secure_boot]):
  secure_boot           auto (default: use the shim chain whenever the image is signed and the machine boots UEFI) | on | off
  secure_boot_key/cert  bring your own machine key: PEM paths (relative to the answer file); default: generate one
  secure_boot_mok_password  one-time MokManager password for the machine key; default: generated and shown at the end
  secure_boot_machine_key   true (default) | false: no machine key (kernel updates must then be signed by hand)
"""
import os
import secrets
import shutil
import subprocess

from . import arch

SB_DIR = "/usr/share/live-installer/secureboot"
EFIVARS = "/sys/firmware/efi/efivars"
SECUREBOOT_VAR = EFIVARS + "/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
PW_ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"      # no 0/o/1/l/i: typed by hand in MokManager (US layout)


def suffix():
    return {"x86_64": "x64", "aarch64": "aa64"}.get(arch.machine(), "")


def material(root="/"):
    """the signed boot files of this image, or None when the image is unsigned (or the arch has no shim)"""
    s = suffix()
    d = os.path.join(root, SB_DIR.lstrip("/"))
    files = {k: os.path.join(d, f) for k, f in (("shim", f"shim{s}.efi"), ("mm", f"mm{s}.efi"), ("grub", f"grub{s}.efi"),
                                                 ("cer", "ENROLL_THIS_KEY_IN_MOKMANAGER.cer"), ("crt", "signing.crt"))}
    if not s or not all(os.path.isfile(p) for p in files.values()):
        return None
    files["suffix"] = s
    return files


def firmware_state():
    """'on' | 'off' | 'unknown' (BIOS boot / no efivars)"""
    try:
        with open(SECUREBOOT_VAR, "rb") as f:
            return "on" if f.read()[-1:] == b"\x01" else "off"
    except OSError:
        return "unknown"


def mode_problem(c, efi, root="/"):
    """validation: SECURE_BOOT=on needs a signed image and UEFI"""
    m = str(c.get("secure_boot", "auto")).lower()
    if m not in ("auto", "on", "off"):
        return f"secure_boot must be auto, on or off (got {m!r})"
    if m == "on" and not efi:
        return "secure_boot = on needs a UEFI boot (this machine booted the installer in BIOS mode)"
    if m == "on" and not material(root):
        return "secure_boot = on, but this image carries no signed boot chain (built without a Secure Boot key)"
    if (c.get("secure_boot_key") or c.get("secure_boot_cert")) and not (c.get("secure_boot_key") and c.get("secure_boot_cert")):
        return "secure_boot: bring-your-own-key needs both key and cert"
    return ""


def active(inst):
    m = str(inst.c.get("secure_boot", "auto")).lower()
    return inst.efi and m != "off" and material() is not None


def esp_grub_cfg(boot_uuid):
    """grub.cfg next to the signed GRUB on the ESP (pure): find /boot by UUID, hand over to its grub.cfg"""
    return ("# written by the installer: the signed GRUB (Secure Boot) reads this file, then /boot/grub/grub.cfg\n"
            f"search --no-floppy --fs-uuid --set=root {boot_uuid}\n"
            "set prefix=($root)/grub\n"
            "configfile ($root)/grub/grub.cfg\n")


def install_esp(inst, esp_dir, boot_uuid):
    """removable-media path EFI/BOOT on the mounted ESP `esp_dir`: shim first, our GRUB next to it, MokManager, certificates"""
    m = material()
    if m is None:
        raise RuntimeError("secure boot: the image carries no signed boot chain")
    s = m["suffix"]
    d = os.path.join(esp_dir, "EFI", "BOOT")
    os.makedirs(d, exist_ok=True)
    for src, name in ((m["shim"], f"BOOT{s.upper()}.EFI"), (m["grub"], f"grub{s}.efi"), (m["mm"], f"mm{s}.efi")):
        shutil.copyfile(src, os.path.join(d, name))
    shutil.copyfile(m["cer"], os.path.join(esp_dir, "ENROLL_THIS_KEY_IN_MOKMANAGER.cer"))
    with open(os.path.join(d, "grub.cfg"), "w") as f:
        f.write(esp_grub_cfg(boot_uuid))
    inst.log(f"secure boot: shim + signed GRUB + MokManager on {esp_dir} (boot fs {boot_uuid})")


def keydir(inst):
    return f"/etc/{inst.p['name']}/secureboot"


def portage_config(kd):
    """(package.use text, make.conf text) that make every update sign with the machine key (pure)"""
    use = ("# written by the installer (Secure Boot): kernels and kernel modules built or installed later are signed with this\n"
           "# machine's key, which MokManager enrolled. Remove together with the make.conf block to stop signing.\n"
           "sys-kernel/gentoo-kernel-bin secureboot\n"
           "sys-kernel/gentoo-kernel secureboot modules-sign\n"
           "sys-kernel/vanilla-kernel secureboot modules-sign\n"
           "*/* modules-sign\n")
    mk = ("\n# Secure Boot machine key (written by the installer; enrolled through MokManager). secureboot.eclass signs EFI\n"
          "# executables (kernels), linux-mod-r1 signs out-of-tree kernel modules (zfs, ...).\n"
          f'SECUREBOOT_SIGN_KEY="{kd}/machine.key"\nSECUREBOOT_SIGN_CERT="{kd}/machine.crt"\n'
          f'MODULES_SIGN_KEY="{kd}/machine.key"\nMODULES_SIGN_CERT="{kd}/machine.crt"\nMODULES_SIGN_HASH="sha512"\n')
    return use, mk


def gen_password(n=8):
    return "".join(secrets.choice(PW_ALPHABET) for _ in range(n))


def machine_key(inst):
    """create (or import) the machine key, point Portage at it, queue its enrollment; returns the MokManager password or ''"""
    c, T = inst.c, inst.target
    if not c.get("secure_boot_machine_key", True):
        inst.log("secure boot: no machine key (secure_boot_machine_key = false): kernels installed later must be signed by hand")
        return ""
    kd = keydir(inst)
    os.makedirs(T + kd, exist_ok=True)
    os.chmod(T + kd, 0o750)
    key, crt, der = T + kd + "/machine.key", T + kd + "/machine.crt", T + kd + "/machine.der"
    if c.get("secure_boot_key"):
        shutil.copyfile(c["secure_boot_key"], key)
        shutil.copyfile(c["secure_boot_cert"], crt)
        inst.log("secure boot: machine key = the key you brought (secure_boot_key)")
    else:
        old = os.umask(0o077)
        try:
            inst.run(["openssl", "req", "-new", "-x509", "-newkey", "rsa:3072", "-sha256", "-nodes", "-days", "36500",
                      "-subj", f"/O={inst.p['title']}/CN={c.get('hostname', inst.p['name'])} Secure Boot machine key",
                      "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "keyUsage=critical,digitalSignature",
                      "-addext", "extendedKeyUsage=codeSigning", "-keyout", key, "-out", crt])
        finally:
            os.umask(old)
    inst.run(["openssl", "x509", "-in", crt, "-outform", "DER", "-out", der])
    # portage builds as the portage user (userpriv): it must read the key; administrators are in the portage group anyway
    inst.run(["chown", "root:portage", kd, key], chroot=True, check=False)
    os.chmod(key, 0o640)
    os.chmod(crt, 0o644)
    os.chmod(der, 0o644)
    use, mk = portage_config(kd)
    inst.write("/etc/portage/package.use/zz-secureboot", use)
    inst.write("/etc/portage/make.conf", mk, append=True)
    # warn at every kernel install while the machine key is not enrolled yet (the new kernel would not boot with SB on)
    inst.write("/etc/kernel/postinst.d/40-secureboot-enrolled",
               "#!/bin/sh\n# written by the installer: kernels signed with the machine key boot under Secure Boot only once it is enrolled\n"
               f"[ -d /sys/firmware/efi ] || exit 0\nmokutil --sb-state 2>/dev/null | grep -q enabled || exit 0\n"
               f"mokutil --test-key {kd}/machine.der 2>/dev/null | grep -q 'already enrolled' && exit 0\n"
               f"echo 'WARNING: Secure Boot is on but the machine key {kd}/machine.der is not enrolled: this kernel will not boot.' >&2\n"
               f"echo '         Enroll it: mokutil --import {kd}/machine.der, reboot, and confirm in MokManager.' >&2\nexit 0\n", 0o755)
    pw = c.get("secure_boot_mok_password") or gen_password()
    if not os.path.isdir(EFIVARS):
        inst.log("secure boot: no EFI variables: the machine key is not queued (run mokutil --import on the installed system)")
        return ""
    h = subprocess.run(["openssl", "passwd", "-6", "-stdin"], input=pw, capture_output=True, text=True, check=True).stdout.strip()
    hf = f"/run/mok-hash-{os.getpid()}"
    old = os.umask(0o077)
    try:
        with open(hf, "w") as f:
            f.write(h)
    finally:
        os.umask(old)
    try:
        rc = inst.run(["mokutil", "--import", der, "--hash-file", hf], check=False)
    finally:
        os.unlink(hf)
    if rc != 0:
        inst.log("secure boot: mokutil --import failed: enroll later with `mokutil --import " + kd + "/machine.der`")
        return ""
    inst.log("secure boot: machine key queued for enrollment: MokManager asks for it at the next boot (one-time password on the final page)")
    return pw


def finish(inst):
    """bootloader stage, Secure Boot path: called after the ESPs are populated"""
    pw = machine_key(inst)
    inst.c["secure_boot_mok_password"] = pw
    inst.c["secure_boot_state"] = firmware_state()


def done_lines(c):
    """rows for the final page (TUI + GUI)"""
    rows = []
    if c.get("secure_boot_used"):
        rows.append(("Secure Boot", "ready (shim + signed GRUB/kernel); firmware Secure Boot is " + c.get("secure_boot_state", "unknown")))
        if c.get("secure_boot_mok_password"):
            rows.append(("MOK password", c["secure_boot_mok_password"]))
    return rows


DONE_NOTE = ("At the next boot a blue MokManager screen asks to enroll this machine's key: choose 'Enroll MOK', 'Continue', 'Yes', "
             "type the MOK password above, then 'Reboot'. Kernel updates are signed with that key. Skipping it is safe: the "
             "system still boots, and `mokutil --import` queues it again.")
