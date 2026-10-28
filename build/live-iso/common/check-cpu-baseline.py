#!/usr/bin/env python3
"""Static guard: release binaries must not contain COMPILER-GENERATED code above the portable baseline
(x86-64-v2 / generic arm64 / rv64gc).

A `-C target-cpu=native` build on an AVX-512 CI host once shipped binaries that died with SIGILL
("invalid opcode") on an Intel machine. The ISO QEMU tests missed it because they ran with -cpu host.

Usage: check-cpu-baseline.py BINARY...   (needs binutils objdump; picks the rule set from the ELF machine)

Why signatures and not "any AVX": dependencies legitimately carry runtime-dispatched AVX2/AVX-512 code (aws-lc,
ring, memchr, ...; thousands of ymm/zmm instructions in a clean x86-64-v2 build). What only a raised
-C target-cpu produces is the scalar BMI/LZCNT family the compiler sprinkles through ordinary Rust code
(blsr, blsi, blsmsk, bzhi, lzcnt, sarx): a clean x86-64-v2 build of patronus-cli/web has 0 of them, the native
build had >1000. Hand-written dispatched assembly does not use them; a small tolerance absorbs the odd one.
Exit 0 = every binary is within the baseline, 1 = violation, 2 = usage/tool error.
"""
import re, subprocess, sys

TOLERANCE = 16   # clean builds measure 0; native builds measure hundreds to thousands

X86_SIGNATURE = {"blsr", "blsi", "blsmsk", "bzhi", "lzcnt", "sarx"}
RE_INSN = re.compile(r"^\s+[0-9a-f]+:\s+(?:(?:lock|rep\w*|bnd|notrack|data16|cs|ds)\s+)*([a-z][a-z0-9.]*)(.*)$")
ARM_SVE = re.compile(r"\bz\d{1,2}\.[bhsd]\b|\bp\d{1,2}/[mz]\b")
RV_BEYOND = {"vsetvli", "vsetivli", "sh1add", "sh2add", "sh3add", "clz", "ctz", "cpop", "andn", "rev8", "bext", "bset", "bclr"}


def machine(path):
    out = subprocess.run(["objdump", "-f", path], capture_output=True, text=True)
    m = re.search(r"file format elf64-(\S+)", out.stdout)
    arch = re.search(r"architecture:\s*([^,\s]+)", out.stdout)
    return (m.group(1) if m else "?"), (arch.group(1) if arch else "?")


def scan(path):
    fmt, arch = machine(path)
    if fmt.startswith("x86-64"):
        kind = "x86_64"
    elif fmt.startswith("littleaarch64"):
        kind = "aarch64"
    elif fmt.startswith("littleriscv"):
        kind = "riscv64"
    else:
        return None, f"{path}: unsupported format {fmt!r}/{arch!r}", {}
    p = subprocess.Popen(["objdump", "-d", "--no-show-raw-insn", path], stdout=subprocess.PIPE, text=True, errors="replace")
    hits, total = {}, 0
    assert p.stdout is not None
    for line in p.stdout:
        m = RE_INSN.match(line)
        if not m:
            continue
        total += 1
        mn, rest = m.groups()
        bad = None
        if kind == "x86_64" and mn in X86_SIGNATURE:
            bad = mn
        elif kind == "aarch64" and ARM_SVE.search(rest):
            bad = "sve:" + mn
        elif kind == "riscv64" and mn in RV_BEYOND:
            bad = mn
        if bad:
            hits[bad] = hits.get(bad, 0) + 1
    if p.wait() != 0 or total == 0:
        return None, f"{path}: objdump failed or found no instructions", {}
    return kind, f"{total} instructions", hits


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    rc = 0
    for path in argv:
        kind, info, hits = scan(path)
        if kind is None:
            print(f"ERROR {info}")
            return 2
        n = sum(hits.values())
        if n > TOLERANCE:
            top = ", ".join(f"{k}={v}" for k, v in sorted(hits.items(), key=lambda kv: -kv[1])[:8])
            print(f"FAIL {path} ({kind}): {n} compiler-generated instructions above the portable baseline [{top}] - built with target-cpu=native?")
            rc = 1
        else:
            print(f"OK   {path} ({kind}): within baseline ({info}; {n} above-baseline signature instructions, limit {TOLERANCE})")
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
