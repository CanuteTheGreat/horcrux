"""Declarative machine definitions (what a test *has*; node.py is what it *does*)."""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Optional


@dataclass
class Disk:
    """A virtio disk.  `size` -> blank qcow2 of that size; `path` -> existing image (qcow2 overlay on top unless `direct`)."""
    size: Optional[str] = "20G"
    path: Optional[str] = None
    fmt: str = "qcow2"
    direct: bool = False          # attach `path` as is (writes persist); default = per-run overlay, base untouched
    readonly: bool = False


@dataclass
class Nic:
    """`net="user"` = QEMU user-mode NAT (DHCP 10.0.2.x, host forwards).  Any other value names a Network."""
    net: str = "user"
    forwards: dict = field(default_factory=lambda: {"ssh": 22})   # label -> guest TCP port (user NICs only)
    mac: Optional[str] = None
    subnet: Optional[str] = None           # user NICs: own NAT subnet (e.g. "10.77.0.0/24", QEMU host = .2); needed for a 2nd user NIC
    guest_ip: Optional[str] = None         # user NICs: guest address the forwards point at (default: the DHCP lease)
    model: str = "virtio-net-pci"


@dataclass
class Network:
    name: str
    kind: str = "hub"             # hub: user-space switch with fault injection; bridge: host bridge (root, no faults)
    bridge: Optional[str] = None
    pcap: bool = True


@dataclass
class NodeSpec:
    name: str
    mem: int = 1024
    cpus: int = 2
    arch: str = "x86_64"
    image: Optional[str] = None           # boot disk (copy-on-write overlay per run)
    image_fmt: str = "qcow2"
    iso: Optional[str] = None             # installer / live media
    iso_media: str = "cdrom"              # cdrom | usb
    extra_drives: list = field(default_factory=list)   # [(path, "usb"|"virtio")] raw drives (e.g. the answer-file volume)
    disks: list = field(default_factory=list)
    nics: list = field(default_factory=lambda: [Nic()])
    firmware: str = "bios"                # bios | uefi
    console: str = "serial-root"          # serial-root: a root shell/login on ttyS0 | none (ssh only)
    login_user: str = "root"
    login_password: Optional[str] = None
    console_menu_key: Optional[str] = None  # appliance console menu after login: the key that opens a root shell (Patronus: "8")
    ssh_key: Optional[str] = None         # private key path for the ssh channel
    ssh_user: str = "root"
    aliases: dict = field(default_factory=dict)   # {"sshd": {"systemd": "ssh"}} per-init unit-name differences
    cpu: str = "qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt,+cx16"   # conservative baseline, never `host` (native-opcode crashes on other CPUs)
    rtc_base: Optional[str] = None
    boot_timeout: int = 300
    extra_args: list = field(default_factory=list)
    vnc: bool = False                     # also listen on a localhost VNC port (node.vnc_port) for a human to watch


@dataclass
class Test:
    name: str
    nodes: list
    run: Callable
    networks: list = field(default_factory=list)
    timeout: int = 3600                   # hard deadline for the whole test (seconds, unscaled)
    description: str = ""
