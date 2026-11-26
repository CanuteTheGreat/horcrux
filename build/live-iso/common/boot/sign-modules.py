#!/usr/bin/env python3
"""sign-modules.py --key K --cert C [--hash sha512] [--force] DIR|FILE...

Appends a kernel module signature (the format the kernel's scripts/sign-file writes: a detached PKCS#7/CMS
signature, then struct module_signature, then the "~Module signature appended~\\n" magic) to every .ko under the
given directories that carries no signature yet. Modules signed at kernel build time (in-tree, signed by the
kernel's own build key) are left alone; out-of-tree modules built into the image (zfs, spl, ...) get ours, so the
kernel loads them when Secure Boot enforces module signatures (IMA arch policy) and our certificate is enrolled
(MOK -> .machine keyring). Compressed modules (.ko.xz/.ko.zst/.ko.gz) are decompressed, signed and recompressed.

Needs only `openssl` (and xz/zstd/gzip for compressed modules): no kernel source tree required.
"""
import argparse
import os
import struct
import subprocess
import sys
import tempfile

MAGIC = b"~Module signature appended~\n"
PKEY_ID_PKCS7 = 2
COMP = {".xz": (["xz", "-dc"], ["xz", "-c", "--check=crc32", "--lzma2=dict=1MiB"]),
        ".zst": (["zstd", "-dcq"], ["zstd", "-cq", "-19"]),
        ".gz": (["gzip", "-dc"], ["gzip", "-cn9"])}


def is_signed(data):
    return data.endswith(MAGIC)


def strip_sig(data):
    """module bytes without an appended signature"""
    if not is_signed(data):
        return data
    sig_len = struct.unpack(">I", data[-len(MAGIC) - 4:-len(MAGIC)])[0]
    return data[:-(len(MAGIC) + 12 + sig_len)]


def sign_bytes(data, key, cert, hash_alg):
    with tempfile.NamedTemporaryFile() as f:
        f.write(data)
        f.flush()
        sig = subprocess.run(["openssl", "cms", "-sign", "-binary", "-noattr", "-nocerts", "-nosmimecap", "-outform", "DER",
                              "-md", hash_alg, "-signer", cert, "-inkey", key, "-in", f.name],
                             check=True, capture_output=True).stdout
    # struct module_signature { u8 algo, hash, id_type, signer_len, key_id_len; u8 pad[3]; be32 sig_len }
    info = struct.pack(">BBBBB3xI", 0, 0, PKEY_ID_PKCS7, 0, 0, len(sig))
    return data + sig + info + MAGIC


def handle(path, a):
    ext = next((e for e in COMP if path.endswith(".ko" + e)), "")
    raw = open(path, "rb").read()
    data = subprocess.run(COMP[ext][0], input=raw, check=True, capture_output=True).stdout if ext else raw
    if is_signed(data) and not a.force:
        return "kept"
    out = sign_bytes(strip_sig(data), a.key, a.cert, a.hash)
    if ext:
        out = subprocess.run(COMP[ext][1], input=out, check=True, capture_output=True).stdout
    st = os.stat(path)
    tmp = path + ".sig-tmp"
    with open(tmp, "wb") as f:
        f.write(out)
    os.chmod(tmp, st.st_mode & 0o7777)
    os.chown(tmp, st.st_uid, st.st_gid) if os.geteuid() == 0 else None
    os.replace(tmp, path)
    return "signed"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--key", required=True)
    ap.add_argument("--cert", required=True)
    ap.add_argument("--hash", default="sha512")
    ap.add_argument("--force", action="store_true", help="re-sign modules that already carry a signature")
    ap.add_argument("paths", nargs="+")
    a = ap.parse_args()
    n = {"signed": 0, "kept": 0}
    for p in a.paths:
        files = [p] if os.path.isfile(p) else [os.path.join(d, f) for d, _, fs in os.walk(p) for f in fs
                                               if f.endswith((".ko", ".ko.xz", ".ko.zst", ".ko.gz"))]
        for f in sorted(files):
            r = handle(f, a)
            n[r] += 1
            if r == "signed":
                print(f"signed {f}")
    print(f"modules: {n['signed']} signed now, {n['kept']} already signed (kept)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
