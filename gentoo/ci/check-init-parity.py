#!/usr/bin/env python3
"""Init parity gate (owner rule 2026-11-13: Horcrux and Patronus fully support OpenRC AND systemd).

For every ebuild under gentoo/:
  1. an ebuild that installs an OpenRC script (newinitd/doinitd) must also install a systemd unit
     (systemd_dounit/systemd_newunit/systemd_install_serviced) and `inherit systemd`;
  2. every FILESDIR/ file an install call names must exist;
  3. no hard dependency on sys-apps/openrc (only inside an `|| ( openrc systemd )` group).
Machine-layer packages that another workstream owns are listed in EXEMPT with the reason; the list may only shrink.
INIT_HALF lists packages that ARE one init system's implementation of something whose other half is a different
package (gnome-session-openrc on OpenRC, upstream gnome-session on systemd): they may depend on their init system,
and every ebuild that pulls them must do so under the matching USE conditional (`!systemd? ( ... )`).
Exit 1 with the whole list of findings.
"""
import os, re, sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
EXEMPT = {
    # HOGSMEADE machine layer: its GNOME-on-OpenRC session is being fixed by another workstream; unit added in phase 2.
    "app-misc/hogsmeade-tools": "hogsmeade machine layer (phase 2)",
}
INIT_HALF = {
    # GNOME's session manager on OpenRC; systemd systems run ::gentoo gnome-base/gnome-session (horcrux-gnome-defaults).
    "gnome-base/gnome-session-openrc": "!systemd?",
}
INIT_CALL = re.compile(r"\b(newinitd|doinitd)\b")
UNIT_CALL = re.compile(r"\b(systemd_dounit|systemd_newunit|systemd_install_serviced)\b")


def ebuilds():
    for cat in sorted(os.listdir(ROOT)):
        cp = os.path.join(ROOT, cat)
        if not os.path.isdir(cp) or cat in ("ci", "profiles", "metadata", "sets", "t2", "surface", "package.use", "package.accept_keywords"):
            continue
        for pn in sorted(os.listdir(cp)):
            pd = os.path.join(cp, pn)
            if os.path.isdir(pd):
                for f in sorted(os.listdir(pd)):
                    if f.endswith(".ebuild"):
                        yield f"{cat}/{pn}", pd, f


def dep_atoms(body):
    """(atom, enclosing USE conditionals) for every atom in the *DEPEND="..." variables."""
    for m in re.finditer(r'^\s*(?:[RPB]?DEPEND|IDEPEND)\+?="([^"]*)"', body, re.M):
        stack, pending = [], None
        for tok in m.group(1).split():
            if tok.endswith("?"):
                pending = tok
            elif tok == "(":
                stack.append(pending)
                pending = None
            elif tok == ")":
                if stack:
                    stack.pop()
            else:
                pending = None
                yield tok, [c for c in stack if c]


def named_files(text):
    # "${FILESDIR}/x" or "${FILESDIR}"/x  (also brace lists: horcrux-test-{a,b}.service)
    for m in re.finditer(r'"?\$\{FILESDIR\}"?/([^\s"\\]+)', text):
        name = m.group(1)
        bm = re.match(r"(.*?)\{([^}]*)\}(.*)", name)
        if bm:
            for alt in bm.group(2).split(","):
                yield from named_files('"${FILESDIR}"/' + bm.group(1) + alt + bm.group(3))
        else:
            yield name


def main():
    bad = []
    for cp, pd, f in ebuilds():
        text = open(os.path.join(pd, f)).read()
        body = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))
        has_init, has_unit = bool(INIT_CALL.search(body)), bool(UNIT_CALL.search(body))
        if has_init and not has_unit and cp not in EXEMPT:
            bad.append(f"{cp}/{f}: installs an OpenRC script but no systemd unit")
        if has_unit and not re.search(r"^inherit .*\bsystemd\b", body, re.M):
            bad.append(f"{cp}/{f}: systemd_dounit without `inherit systemd`")
        if cp in EXEMPT and has_unit:
            bad.append(f"{cp}: has a unit now, drop it from EXEMPT in check-init-parity.py")
        for name in set(named_files(body)):
            if not re.search(r"[$*]", name) and not os.path.exists(os.path.join(pd, "files", name)):
                bad.append(f"{cp}/{f}: names files/{name} which does not exist")
        # an init-specific half must only be pulled under its USE conditional
        for half, cond in INIT_HALF.items():
            if cp != half:
                for atom, stack in dep_atoms(body):
                    if half in atom and cond not in stack:
                        bad.append(f"{cp}/{f}: pulls {half} outside `{cond} ( ... )`")
        if cp in INIT_HALF:
            continue
        # hard openrc dependency: a dependency line (tab-indented atom) outside an || ( ) group
        for m in re.finditer(r"^\s+(?!\|\|)(\S*sys-apps/openrc)\s*$", body, re.M):
            bad.append(f"{cp}/{f}: hard dependency on sys-apps/openrc (use || ( sys-apps/openrc sys-apps/systemd ))")
        for m in re.finditer(r"^\s*[RP]?DEPEND=\"[^\n\"]*sys-apps/openrc", body, re.M):
            bad.append(f"{cp}/{f}: hard dependency on sys-apps/openrc")
    if bad:
        print("init parity check FAILED:\n  " + "\n  ".join(sorted(set(bad))))
        return 1
    print("init parity: every ebuild that ships an OpenRC script ships a systemd unit (exempt: %s)" % ", ".join(EXEMPT))
    return 0


sys.exit(main())
