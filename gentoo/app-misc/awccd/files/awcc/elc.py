"""ELC (AlienFX v4, USB 187C:0551) lighting over hidraw, per awcc-re/alienfx-protocol.md section 2 (34-byte frames,
report id 0). Frame builders are pure (unit tested); the transport probes Output-vs-Feature framing once.

Safety: only the commands of section 2 that were exercised in the owner-watched tests are built here. 0xFF (erase SPI
flash) and firmware update are deliberately NOT implemented and `frame()` refuses them."""
import fcntl
import glob
import os
import select
import time

VID, PIDS = 0x187C, (0x0551, 0x0550)
FRAME = 34
FORBIDDEN_CMDS = (0xFF,)
ZONES = {"shelf": list(range(0, 31)), "power": [31], "alienhead": [32], "fan2": [33], "fan1": [34], "touchpad": list(range(35, 45))}
ALL_LEDS = list(range(0, 45))
RAINBOW_WAVE = 16                    # firmware predefined animation AWCC ships as the default preset


def frame(cls, cmd, args=b""):
    """one 34-byte frame (byte 0 = report id 0): 00 <class> <cmd> <args...>"""
    if cmd in FORBIDDEN_CMDS:
        raise ValueError(f"command 0x{cmd:02X} is destructive and never sent")
    b = bytes([0, cls, cmd]) + bytes(args)
    if len(b) > FRAME:
        raise ValueError("frame too long")
    return b + bytes(FRAME - len(b))


def q_firmware():
    return frame(3, 0x20, b"\x00")


def q_config():
    return frame(3, 0x20, b"\x02")


def anim_cmd(sub, anim_id, duration=None):
    """0x21: sub 1 START_NEW, 2 FINISH_AND_SAVE (never used), 3 FINISH_AND_PLAY, 4 REMOVE, 5 PLAY"""
    if sub in (2, 6, 7):
        raise ValueError("persistent animation commands (save/default/startup) are not used")
    a = bytes([(sub >> 8) & 255, sub & 255, (anim_id >> 8) & 255, anim_id & 255])
    if duration is not None:
        a += bytes([(duration >> 8) & 255, duration & 255])
    return frame(3, 0x21, a)


def start_series(leds, loop=True):
    """0x23 chunks of up to 28 LED ids"""
    out = []
    for i in range(0, len(leds), 28):
        chunk = leds[i:i + 28]
        out.append(frame(3, 0x23, bytes([1 if loop else 0, i // 28, len(chunk)]) + bytes(chunk)))
    return out


def action(effect, duration_ms, tempo, rgb):
    """0x24 with one action: effect Color 0, Pulse 1, Morph 2, Aurora 3, Gradient 4"""
    r, g, b = rgb
    return frame(3, 0x24, bytes([effect, (duration_ms >> 8) & 255, duration_ms & 255, (tempo >> 8) & 255, tempo & 255, r, g, b]))


def dim(level, leds):
    out = []
    for i in range(0, len(leds), 28):
        chunk = leds[i:i + 28]
        out.append(frame(3, 0x26, bytes([min(100, level), i // 28, len(chunk)]) + bytes(chunk)))
    return out


def static_color_sequence(leds, rgb):
    """the VERIFIED static-colour recipe (alienfx-protocol.md, evidence E060): replace the running animation with a looped
    one-action user animation. A plain SetColor is overwritten by the firmware animation (flicker only)."""
    seq = [anim_cmd(1, 0xFFFF)]
    seq += start_series(leds, loop=True)
    seq.append(action(0, 2000, 250, rgb))
    seq.append(anim_cmd(3, 0x00FF))
    return seq


def restore_sequence(anim_id=RAINBOW_WAVE):
    """PLAY the predefined animation, then REMOVE the temporary ones (id travels as 0xFFFF for START, (byte) for the rest)"""
    return [anim_cmd(5, anim_id), anim_cmd(4, 0x00FF), anim_cmd(4, 0xFFFF)]


def parse_hex_color(s):
    s = s.strip().lstrip("#")
    if len(s) != 6:
        raise ValueError("colour must be RRGGBB")
    return tuple(int(s[i:i + 2], 16) for i in (0, 2, 4))


def zones_to_leds(names):
    leds = []
    for n in names:
        if n == "all":
            return list(ALL_LEDS)
        if n not in ZONES:
            raise ValueError(f"unknown zone {n!r} (one of {', '.join(ZONES)}, all)")
        leds += [x for x in ZONES[n] if x not in leds]
    return leds


def find_hidraw(sysroot="/"):
    """/dev/hidrawN of the ELC (HID_ID 0003:0000187C:00000551), or None"""
    for d in sorted(glob.glob(os.path.join(sysroot, "sys/class/hidraw/hidraw*"))):
        try:
            with open(os.path.join(d, "device/uevent")) as f:
                ue = f.read().upper()
        except OSError:
            continue
        for pid in PIDS:
            if f"HID_ID=0003:{VID:08X}:{pid:08X}" in ue:
                return "/dev/" + os.path.basename(d)
    return None


def _hid_ioc(rw, nr, size):          # include/uapi/linux/hidraw.h: HIDIOCSFEATURE(len)=_IOC(RW,'H',0x06,len), GFEATURE 0x07
    return (rw << 30) | (size << 16) | (ord("H") << 8) | nr


class ELC:
    """transport: tries OUTPUT framing (plain hidraw write, what Windows HidD_SetOutputReport does) first, then FEATURE (OpenRGB);
    the first one whose firmware-version query answers `83 20 00` is remembered."""

    def __init__(self, path=None, sysroot="/"):
        self.path = path or find_hidraw(sysroot)
        self.fd = None
        self.method = None

    def open(self):
        if not self.path:
            raise OSError("ELC (187C:0551) hidraw node not found")
        self.fd = os.open(self.path, os.O_RDWR | os.O_NONBLOCK)

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None

    def _send_out(self, fr):
        os.write(self.fd, fr[1:])         # report id 0 is not transmitted on hidraw for id-less devices

    def _send_feat(self, fr):
        buf = bytearray(fr)
        fcntl.ioctl(self.fd, _hid_ioc(3, 0x06, len(buf)), buf)

    def _read_in(self, timeout=0.2):
        r, _, _ = select.select([self.fd], [], [], timeout)
        return os.read(self.fd, 64) if r else b""

    def _read_feat(self):
        buf = bytearray(FRAME)
        try:
            fcntl.ioctl(self.fd, _hid_ioc(3, 0x07, len(buf)), buf)
        except OSError:
            return b""
        return bytes(buf)

    def xfer(self, fr, method=None):
        method = method or self.method or "output"
        self._drain()
        (self._send_out if method == "output" else self._send_feat)(fr)
        time.sleep(0.03)
        return self._read_in() if method == "output" else self._read_feat()

    def _drain(self):
        while select.select([self.fd], [], [], 0)[0]:
            os.read(self.fd, 64)

    def probe(self):
        """pick the framing that answers the read-only firmware query; returns (method, version) or raises"""
        for m in ("output", "feature"):
            try:
                rep = self.xfer(q_firmware(), m)
            except OSError:
                continue
            r = rep[1:] if rep[:1] == b"\x00" and len(rep) > 3 and rep[1] in (0x83, 0x03) else rep
            if len(r) >= 6 and r[0] == 0x83 and r[1] == 0x20:
                self.method = m
                return m, f"{r[3]}.{r[4]}.{r[5]}"
        raise OSError("ELC did not answer the firmware query in output or feature framing")

    def play(self, frames):
        bad = []
        for fr in frames:
            rep = self.xfer(fr)
            r = rep[1:] if rep[:1] == b"\x00" and len(rep) > 3 else rep
            if not (r[:1] and r[0] & 0x80):
                bad.append(fr[2:4].hex())
        return bad

    def static_color(self, leds, rgb):
        return self.play(static_color_sequence(leds, rgb))

    def restore(self, anim_id=RAINBOW_WAVE):
        return self.play(restore_sequence(anim_id))
