"""udisks_gate.py: QEMU gate for horcrux-core's 80-horcrux-udisks-hide.rules (system storage hidden from udisks2 / GNOME Files, a user's vfat stick visible).

Guest side: a loop disk with
  p1 vfat label ESP but GPT name "stick"  (a user's stick: must stay visible)
  p2 installer-style ESP2 (GPT type ef00, name ESP, label ESP2)   -> must be ignored
  p3 installer-style boot2 (ext4, GPT name boot, label boot2)     -> must be ignored
`udevadm info` must show UDISKS_IGNORE=1 on p2/p3 only; where udisks2 is installed, `udisksctl dump` must show HintIgnore=true for p2/p3, false for p1.
"""
import re

CHECK = r'''
[ -f /usr/share/horcrux/core-release ] || { echo "NO-HORCRUX-CORE"; echo "UDISKS-DONE"; exit 0; }
[ -f /usr/lib/udev/rules.d/80-horcrux-udisks-hide.rules ] && echo "RULES 1" || echo "RULES 0"
for t in sgdisk mkfs.vfat mkfs.ext4 losetup; do command -v $t >/dev/null || { echo "NO-TOOLS $t"; echo "UDISKS-DONE"; exit 0; }; done
f=$(mktemp /var/tmp/hxudisks.XXXXXX); truncate -s 200M "$f"
sgdisk -n 1:0:+40M -t 1:0700 -c 1:stick -n 2:0:+40M -t 2:ef00 -c 2:ESP -n 3:0:+40M -t 3:8300 -c 3:boot "$f" >/dev/null 2>&1
d=$(losetup -fP --show "$f") || { echo "NO-LOOP"; echo "UDISKS-DONE"; rm -f "$f"; exit 0; }
udevadm settle --timeout=20 >/dev/null 2>&1; sleep 1
mkfs.vfat -F32 -n ESP ${d}p1 >/dev/null 2>&1; mkfs.vfat -F32 -n ESP2 ${d}p2 >/dev/null 2>&1; mkfs.ext4 -F -q -L boot2 ${d}p3 >/dev/null 2>&1
udevadm trigger --subsystem-match=block --action=change; udevadm settle --timeout=30 >/dev/null 2>&1; sleep 2
for p in 1 2 3; do echo "IGN p$p $(udevadm info -q property ${d}p$p 2>/dev/null | grep -c '^UDISKS_IGNORE=1')"; done
if command -v udisksctl >/dev/null && udisksctl dump >/dev/null 2>&1; then
  for p in 1 2 3; do echo "HINT p$p $(udisksctl dump 2>/dev/null | awk -v dev="$(basename $d)p$p" '/^\/org\/freedesktop\/UDisks2\/block_devices\//{blk=($0 ~ dev"$")} blk && /HintIgnore:/{print $2; exit}')"; done
fi
losetup -d $d; rm -f "$f"
echo "UDISKS-DONE"
'''


def decide(o):
    """(ok, skipped) from the guest output"""
    o = (o or "").replace("\r", "")
    rules = re.search(r"(?m)^RULES 1$", o) is not None
    skipped = bool(re.search(r"(?m)^NO-(TOOLS|LOOP)", o))
    ign = dict(re.findall(r"(?m)^IGN (p\d) (\d+)", o))
    if re.search(r"(?m)^NO-HORCRUX-CORE", o):      # Patronus: no horcrux-core, no desktop, nothing to hide
        return "UDISKS-DONE" in o, True
    ok = "UDISKS-DONE" in o and rules and (skipped or (ign.get("p1") == "0" and ign.get("p2") == "1" and ign.get("p3") == "1"))
    for p, v in re.findall(r"(?m)^HINT (p\d) (\S*)", o):
        ok = ok and (v == ("false" if p == "p1" else "true"))
    return ok, skipped


def gate(sh, step, b64):
    """sh(cmd, timeout) -> (rc, out); b64(interp, text) wraps a script for ssh (headless_gates.b64)"""
    rc, o = sh(b64("sh", CHECK), 120)
    ok, skipped = decide(o)
    flat = re.sub(r"\s+", " ", (o or "").strip())
    step("udisks-hides-system-storage-keeps-usb-vfat", ok,
         "rules installed; loop-disk probe skipped (" + flat[-80:] + ")" if ok and skipped else flat[-300:])
    return ok
