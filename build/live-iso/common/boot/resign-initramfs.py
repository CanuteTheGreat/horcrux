#!/usr/bin/env python3
"""resign-initramfs.py --key K --cert C INITRAMFS...

dracut images are [optional uncompressed early cpio (microcode)] + [compressed main cpio]. The main archive carries copies
of the kernel modules the initramfs loads (zfs/spl for root on ZFS): built before the image was signed, those copies are
unsigned and a Secure Boot kernel (IMA arch policy -> module signatures enforced) rejects them. This unpacks the main
archive, signs every unsigned .ko with sign-modules.py, repacks it with the same compressor and puts the early cpio back.
An image with nothing to sign is left byte-identical.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
MAGICS = [(b"\x28\xb5\x2f\xfd", ["zstd", "-dcq"], ["zstd", "-cq", "-15", "-T0"]),
          (b"\xfd7zXZ\x00", ["xz", "-dc"], ["xz", "-c", "--check=crc32", "-T0"]),
          (b"\x1f\x8b", ["gzip", "-dc"], ["gzip", "-cn9"]),
          (b"070701", ["cat"], ["cat"])]


def early_len(data):
    """length of a leading uncompressed newc cpio (dracut's early microcode archive) incl. its zero padding; 0 if none.
    Same rule as dracut's skipcpio: walk the headers to TRAILER!!!, then skip NUL padding."""
    if not data.startswith(b"070701"):
        return 0
    off = 0
    while data[off:off + 6] == b"070701":
        namesize = int(data[off + 94:off + 102], 16)
        filesize = int(data[off + 54:off + 62], 16)
        name = data[off + 110:off + 110 + namesize - 1]
        off = (off + 110 + namesize + 3) & ~3
        off = (off + filesize + 3) & ~3
        if name == b"TRAILER!!!":
            break
    while off < len(data) and data[off] == 0:
        off += 1
    return off if off < len(data) else 0      # an image that is ONE uncompressed cpio: no early part


def split(img, _skipcpio=""):
    data = open(img, "rb").read()
    n = early_len(data)
    return data[:n], data[n:]


def run(img, a):
    early, main = split(img, a.skipcpio)
    for magic, dec, enc in MAGICS:
        if main.startswith(magic):
            break
    else:
        print(f"{img}: unknown compression {main[:6]!r}; left alone", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory() as d:
        cpio = subprocess.run(dec, input=main, check=True, capture_output=True).stdout
        subprocess.run(["cpio", "-idm", "--quiet", "--no-absolute-filenames"], input=cpio, cwd=d, check=True)
        out = subprocess.run([sys.executable, os.path.join(HERE, "sign-modules.py"), "--key", a.key, "--cert", a.cert, d],
                             check=True, capture_output=True, text=True).stdout
        if " 0 signed now" in out:
            print(f"{img}: all modules already signed")
            return 0
        names = subprocess.run("find . -mindepth 1 | LC_ALL=C sort", shell=True, cwd=d, check=True, capture_output=True).stdout
        newcpio = subprocess.run(["cpio", "-o", "-H", "newc", "--quiet", "--reproducible"], input=names, cwd=d, check=True,
                                 capture_output=True).stdout
    packed = subprocess.run(enc, input=newcpio, check=True, capture_output=True).stdout
    tmp = img + ".sbtmp"
    with open(tmp, "wb") as f:
        f.write(early + packed)
    shutil.copymode(img, tmp)
    os.replace(tmp, img)
    print(f"{img}: {out.strip().splitlines()[-1]}")
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--key", required=True)
    ap.add_argument("--cert", required=True)
    ap.add_argument("--skipcpio", default="", help=argparse.SUPPRESS)
    ap.add_argument("images", nargs="+")
    a = ap.parse_args()
    return max(run(i, a) for i in a.images)


if __name__ == "__main__":
    sys.exit(main())
