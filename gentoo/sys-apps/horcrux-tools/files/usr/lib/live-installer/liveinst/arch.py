"""Per-architecture facts for the installer (x86_64, aarch64, riscv64).

Everything that differs between CPU architectures lives here, so the stages never test
`uname -m` themselves: boot method (UEFI everywhere, legacy BIOS only on x86), the GRUB
platform, and the serial console the kernel was started with.
"""
import os
import platform
import re

_MACHINE = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64", "riscv64": "riscv64"}

# GRUB EFI target, removable-media boot file name, legacy BIOS possible?
_BOOT = {
    "x86_64": dict(efi_target="x86_64-efi", efi_file="BOOTX64.EFI", bios=True, grub_serial=True),
    "aarch64": dict(efi_target="arm64-efi", efi_file="BOOTAA64.EFI", bios=False, grub_serial=False),
    "riscv64": dict(efi_target="riscv64-efi", efi_file="BOOTRISCV64.EFI", bios=False, grub_serial=False),
}
_SERIAL_RE = re.compile(r"(?:^|\s)console=(tty(?:S|AMA|AML|MXC|USB)\d+|hvc\d+)(?:,(\d+)\S*)?")


def machine():
    m = platform.machine().lower()
    return _MACHINE.get(m, m)


def boot(m=None):
    """boot facts for architecture m (default: this machine)"""
    return _BOOT.get(m or machine(), _BOOT["x86_64"])


def has_bios(m=None):
    return boot(m)["bios"]


def serial_console(cmdline=None):
    """(device, baud) of the serial console on the kernel command line, e.g. ('ttyAMA0', 115200); None without one"""
    if cmdline is None:
        try:
            cmdline = open("/proc/cmdline").read()
        except OSError:
            return None
    m = _SERIAL_RE.search(cmdline)
    if not m:
        return None
    return m.group(1), int(m.group(2) or 115200)


def console_args(cmdline=None):
    """kernel arguments that keep the serial console alive on the installed system ('' when there is none)"""
    s = serial_console(cmdline)
    return f"console=tty0 console={s[0]},{s[1]}n8" if s else ""


def grub_serial_lines(cmdline=None, m=None):
    """GRUB serial terminal setup (x86 only: on arm64/riscv64 the EFI console already goes to the UART)"""
    s = serial_console(cmdline)
    if not s or not boot(m)["grub_serial"]:
        return None
    return f"serial --unit={int(re.sub(r'^tty[A-Z]*', '', s[0]) or 0)} --speed={s[1]}"
