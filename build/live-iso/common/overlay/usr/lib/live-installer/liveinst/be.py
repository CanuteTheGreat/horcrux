"""Boot environments (BE) for a ZFS-root Horcrux: `horcrux-be`.

Layout (pool `rpool`, written by the installer, see zfsroot.py):
  rpool/ROOT/<name>   root file systems (mountpoint=/, canmount=noauto), one per boot environment
  rpool/home, rpool/var/log, rpool/var/lib/libvirt   shared datasets (not rolled back with the root)
  /boot (unencrypted ext4):  be/<name>/{vmlinuz,initramfs.img,kver}   kernel of each BE
                              be/<name>/kernels/<kver>/...           the previous kernel(s) kept bootable (KERNELS_KEEP, default 2 incl. current)
                              be/<name>/good                         kver of the last kernel that booted successfully (`mark-good`)
                              be/.snap/<snapshot>/...                kernel saved with each snapshot
                              be/.objects/<sha256>                   content store: every kernel/initramfs file in be/ is a
                                                                     HARD LINK to one object, so identical kernels cost nothing
                                                                     extra and an object nobody links to any more is deleted
                              grub/grub.cfg                          generated here (static: GRUB cannot read encrypted ZFS)

A BE = a dataset + the kernel/initramfs that match its /lib/modules. `snapshot` records both, `create --from`
clones a snapshot into a new BE and restores its kernel, `activate` sets the pool's `bootfs` and the GRUB default.
Everything is non-destructive: the previous BE stays in the GRUB menu.
"""
import argparse
import fnmatch
import glob
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time

CONF = "/etc/horcrux/be.conf"
PIN_PROP = "horcrux:pin"  # ZFS user property on a snapshot or boot environment: "yes" = never pruned/destroyed automatically
BOOT = "/boot"
OBJECTS = ".objects"
BIG = 64 * 1024  # files below this are copied, not stored as objects
AUTO_SNAP = re.compile(r"(pre-upgrade|create)-\d{8}-\d{6}")  # created by horcrux-be itself; anything else is the user's


def conf(path=CONF):
    c = {"POOL": "rpool", "KEEP": "5", "AUTO_SNAPSHOT": "yes", "TIMEOUT": "5", "CMDLINE": "", "KERNEL_GLOB": "*",
         "BOOT_MAX_USED_PCT": "80", "BOOT_MIN_FREE_MB": "32",
         # kernels kept bootable per BE (current + previous ones); a new kernel boots ONCE as a trial (grub next_entry) and becomes the
         # default only after `mark-good` ran in that boot, otherwise the next boot is the last known-good kernel again
         "KERNELS_KEEP": "2", "KERNEL_TRIAL": "yes", "GOOD_DELAY": "60", "BOOT_LOADER": "grub", "GRUBENV": "",
         "MODULES_DIR": "/lib/modules",
         # directories that hold user data / VM disks: they MUST be their own datasets (outside rpool/ROOT), or a rollback would lose them
         "DATA_PATHS": "/home /var/lib/libvirt /var/lib/horcrux/vms /var/lib/docker /var/lib/lxc /var/lib/incus /var/lib/lxd"}
    try:
        for line in open(path):
            m = re.match(r'^\s*([A-Z_]+)\s*=\s*"?([^"\n]*)"?\s*$', line)
            if m:
                c[m.group(1)] = m.group(2)
    except OSError:
        pass
    return c


def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=isinstance(cmd, str), capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"horcrux-be: {cmd if isinstance(cmd, str) else ' '.join(cmd)}: {r.stderr.strip() or r.stdout.strip()}")
    return r.stdout.strip()


def valid_name(n):
    return bool(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,40}", n or "")) and "@" not in n


def be_datasets(pool):
    out = sh(["zfs", "list", "-H", "-o", "name", "-r", "-d", "1", f"{pool}/ROOT"], check=False)
    return [l.split("/")[-1] for l in out.splitlines() if l.count("/") == 2]


def active_be(pool):
    bf = sh(["zpool", "get", "-H", "-o", "value", "bootfs", pool], check=False)
    return bf.split("/")[-1] if bf.startswith(pool + "/ROOT/") else None


def running_be():
    """the BE this system booted from (root=ZFS= on the command line), else the pool's bootfs"""
    m = re.search(r"root=ZFS=\S+/ROOT/([^\s/]+)", open("/proc/cmdline").read()) if os.path.exists("/proc/cmdline") else None
    return m.group(1) if m else None


def vkey(kver):
    """sort key for kernel versions: numbers compare as numbers, text as text (never int vs str)"""
    return [(0, int(x), "") if x.isdigit() else (1, 0, x) for x in re.split(r"(\d+)", kver) if x != ""]


def loose_kernels(boot=BOOT, glob_="*"):
    """[(kver, vmlinuz, initramfs)] newest first: every vmlinuz-<kver> + initramfs-<kver>.img pair installkernel/dracut left in /boot
    (*.old backups are not kernels)"""
    res = []
    for k in glob.glob(os.path.join(boot, "vmlinuz-*")):
        kver = os.path.basename(k)[len("vmlinuz-"):]
        ird = os.path.join(boot, f"initramfs-{kver}.img")
        if kver.endswith((".old", ".new", ".bak")) or not fnmatch.fnmatch(kver, glob_) or not os.path.exists(ird):
            continue
        res.append((kver, k, ird))
    return sorted(res, key=lambda t: vkey(t[0]), reverse=True)


def kernel_files(boot=BOOT, glob_="*"):
    """newest kernel + initramfs installed by installkernel/dracut in /boot (vmlinuz-<kver>, initramfs-<kver>.img)"""
    ks = loose_kernels(boot, glob_)
    return ks[0] if ks else None


def running_kernel():
    return os.uname().release


def say(msg):
    print(f"horcrux-be: {msg}", file=sys.stderr)


def file_hash(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def objdir(boot):
    return os.path.join(boot, "be", OBJECTS)


def link_into(src, dest):
    """make dest a hard link to src (atomically replacing dest); copy when the file system has no hard links"""
    tmp = dest + ".new"
    try:
        os.unlink(tmp)
    except OSError:
        pass
    try:
        if os.path.samefile(src, dest):
            return  # already a link to it (rename of two links to one inode would leave the temp name behind)
    except OSError:
        pass
    try:
        os.link(src, tmp)
    except OSError:
        shutil.copyfile(src, tmp)
        shutil.copymode(src, tmp)
    os.replace(tmp, dest)
    try:
        os.unlink(tmp)  # no-op unless tmp and dest were hard links to the same inode
    except OSError:
        pass


def put_object(boot, src):
    """path of the content-store object holding src's bytes (stored once, whatever links to it)"""
    od = objdir(boot)
    os.makedirs(od, exist_ok=True)
    st = os.stat(src)
    for n in os.listdir(od):  # already one of ours? (same inode) no hashing needed
        try:
            if os.stat(os.path.join(od, n)).st_ino == st.st_ino and not n.endswith(".tmp"):
                return os.path.join(od, n)
        except OSError:
            pass
    obj = os.path.join(od, file_hash(src))
    if not os.path.exists(obj) or os.path.getsize(obj) != st.st_size:
        tmp = obj + ".tmp"
        try:
            shutil.copyfile(src, tmp)
            shutil.copymode(src, tmp)
            os.replace(tmp, obj)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
    return obj


def place_file(boot, src, dest):
    """copy src to dest; big files go through the content store so identical contents share one inode"""
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    if os.path.getsize(src) >= BIG:
        link_into(put_object(boot, src), dest)
        return
    tmp = dest + ".new"
    shutil.copyfile(src, tmp)
    os.replace(tmp, dest)


def copy_tree(boot, src, dest):
    """what shutil.copytree did, but every big file is a link into the store"""
    os.makedirs(dest, exist_ok=True)
    for n in sorted(os.listdir(src)):
        p = os.path.join(src, n)
        if os.path.isfile(p):
            place_file(boot, p, os.path.join(dest, n))


def gc_objects(boot):
    """delete store objects nobody links to any more (st_nlink == 1) and stale temp files; returns bytes freed"""
    freed = 0
    od = objdir(boot)
    try:
        names = os.listdir(od)
    except OSError:
        return 0
    for n in names:
        p = os.path.join(od, n)
        try:
            st = os.stat(p)
            if n.endswith(".tmp") and time.time() - st.st_mtime < 3600:
                continue
            if st.st_nlink <= 1 or n.endswith(".tmp"):
                os.unlink(p)
                freed += st.st_size
        except OSError:
            pass
    return freed


def dedupe(boot=BOOT):
    """turn plain copies under /boot/be (older versions of this tool copied every snapshot) into store links; returns bytes saved"""
    saved = 0
    root = os.path.join(boot, "be")
    for dp, dns, fns in os.walk(root):
        dns[:] = [d for d in dns if d != OBJECTS]
        for n in fns:
            p = os.path.join(dp, n)
            try:
                st = os.stat(p)
                if not os.path.isfile(p) or st.st_nlink > 1 or st.st_size < BIG or n.endswith((".new", ".tmp")):
                    continue
                obj = os.path.join(objdir(boot), file_hash(p))
                if os.path.exists(obj):
                    if os.stat(obj).st_ino == st.st_ino:
                        continue
                    link_into(obj, p)
                    saved += st.st_size
                else:
                    os.makedirs(objdir(boot), exist_ok=True)
                    os.link(p, obj)  # the file itself becomes the object (second link): nothing is copied
            except OSError:
                continue
    gc_objects(boot)
    return saved


def boot_space(boot=BOOT):
    st = os.statvfs(boot)
    return st.f_blocks * st.f_frsize, st.f_bavail * st.f_frsize


def space_ok(c, need, boot=BOOT):
    """would writing `need` more bytes keep /boot under BOOT_MAX_USED_PCT (and leave BOOT_MIN_FREE_MB)?"""
    total, free = boot_space(boot)
    if not total:
        return True
    keep = max(total * (100 - int(c.get("BOOT_MAX_USED_PCT", "80"))) // 100, int(c.get("BOOT_MIN_FREE_MB", "32")) << 20)
    return free - need >= keep


def pinned(pool):
    """names ('be' or 'be@snap') carrying horcrux:pin=yes"""
    r = subprocess.run(["zfs", "get", "-H", "-r", "-t", "snapshot,filesystem", "-o", "name,value", PIN_PROP, f"{pool}/ROOT"],
                       capture_output=True, text=True)
    pre = f"{pool}/ROOT/"
    return {l.split("\t")[0][len(pre):] for l in r.stdout.splitlines() if l.startswith(pre) and l.endswith("\tyes")} if r.returncode == 0 else set()


def cmd_pin(c, name, on=True):
    pool = c["POOL"]
    be = name.partition("@")[0]
    if be not in be_datasets(pool):
        raise SystemExit(f"horcrux-be: no boot environment {be}")
    if "@" in name and name.partition("@")[2] not in {s for b, s, _ in (snapshots(pool) or []) if b == be}:
        raise SystemExit(f"horcrux-be: no snapshot {name}")
    sh(["zfs", "set" if on else "inherit", *([f"{PIN_PROP}=yes"] if on else [PIN_PROP]), f"{pool}/ROOT/{name}"])
    print(f"{'pinned' if on else 'unpinned'} {name}")


def data_isolation(c):
    """For each DATA_PATHS entry: the dataset it lives on and whether a rollback is safe for it. A path that exists on a boot
    environment's own dataset holds data that a rollback would discard; a missing path is fine."""
    pool, res = c["POOL"], []
    for path in c.get("DATA_PATHS", "").split():
        if not os.path.isdir(path):
            res.append({"path": path, "dataset": None, "safe": True, "why": "absent"})
            continue
        r = subprocess.run(["findmnt", "-no", "SOURCE", "-T", path], capture_output=True, text=True)
        src = r.stdout.strip().splitlines()[0] if r.stdout.strip() else ""
        own = src.startswith(f"{pool}/ROOT/") or not src.startswith(f"{pool}/")
        empty = not os.listdir(path)
        res.append({"path": path, "dataset": src or None, "safe": (not own) or empty,
                    "why": "own dataset" if not own else ("empty, on the boot environment" if empty else "HOLDS FILES ON THE BOOT ENVIRONMENT DATASET")})
    return res


def isolation_guard(c, force=False):
    bad = [d for d in data_isolation(c) if not d["safe"]]
    if bad and not force:
        raise SystemExit("horcrux-be: refusing: " + ", ".join(d["path"] for d in bad) + " live on the boot environment dataset itself, so booting "
                         "another environment would hide (not delete, but cut you off from) their current contents. Move them to their own "
                         "datasets (see docs/design/boot-environments.md), or pass --force.")
    return bad


def origins(pool):
    """snapshots that a boot environment was cloned from (they cannot be destroyed while the clone lives)"""
    out = sh(["zfs", "list", "-H", "-o", "origin", "-r", f"{pool}/ROOT"], check=False)
    pre = f"{pool}/ROOT/"
    return {l[len(pre):] for l in out.splitlines() if l.startswith(pre)}


def snap_dir(boot, be, snap):
    return os.path.join(boot, "be", ".snap", f"{be}@{snap}")


def reclaimable(boot, dirs):
    """bytes that disappear from /boot if all of `dirs` (snapshot copies) were removed"""
    inside, size = {}, {}
    for d in dirs:
        for dp, _dns, fns in os.walk(d):
            for n in fns:
                try:
                    st = os.stat(os.path.join(dp, n))
                except OSError:
                    continue
                inside[st.st_ino] = inside.get(st.st_ino, 0) + 1
                size[st.st_ino] = (st.st_size, st.st_nlink)
    total = 0
    for ino, (sz, nl) in size.items():
        # links left = nl - inside; the store's own link may or may not exist (plain legacy copies: none)
        if nl - inside[ino] <= 1:
            total += sz
    return total


def make_room(c, need, boot=BOOT, protect=()):
    """free /boot by destroying the OLDEST automatic snapshots (pre-upgrade-*/create-*, with their /boot copy) until `need` bytes
    fit under the usage limit. Never touches user-named snapshots, boot environments, snapshots a BE was cloned from, or `protect`.
    Returns True when there is room."""
    gc_objects(boot)
    if space_ok(c, need, boot):
        return True
    pool = c["POOL"]
    sn = snapshots(pool)
    if sn is None:
        return False
    org = origins(pool) | pinned(pool)
    cand = [(be, s) for be, s, _cr in sn if AUTO_SNAP.fullmatch(s) and f"{be}@{s}" not in org and f"{be}@{s}" not in protect]
    total, free = boot_space(boot)
    if free + reclaimable(boot, [snap_dir(boot, b, s) for b, s in cand]) - need < max(total * (100 - int(c.get("BOOT_MAX_USED_PCT", "80"))) // 100, int(c.get("BOOT_MIN_FREE_MB", "32")) << 20):
        return False  # destroying every automatic snapshot would not make room: keep them, skip the /boot copy instead
    for be, s in cand:  # oldest first
        say(f"/boot is {100 - 100 * boot_space(boot)[1] // max(boot_space(boot)[0], 1)}% full: removing the oldest automatic snapshot {be}@{s}")
        destroy_snapshot(c, be, s, boot)
        gc_objects(boot)
        if space_ok(c, need, boot):
            return True
    return space_ok(c, need, boot)


def destroy_snapshot(c, be, snap, boot=BOOT):
    """destroy the ZFS snapshot together with its /boot copy; returns False when ZFS refuses (e.g. a clone depends on it)"""
    r = subprocess.run(["zfs", "destroy", f"{c['POOL']}/ROOT/{be}@{snap}"], capture_output=True, text=True)
    if r.returncode != 0:
        say(f"cannot destroy {be}@{snap}: {r.stderr.strip()}")
        return False
    shutil.rmtree(snap_dir(boot, be, snap), ignore_errors=True)
    gc_objects(boot)
    return True


def save_kernel(dest_dir, kver, kimg, initrd, boot=None):
    boot = boot or os.path.dirname(os.path.dirname(dest_dir.rstrip("/")))
    os.makedirs(dest_dir, exist_ok=True)
    place_file(boot, kimg, os.path.join(dest_dir, "vmlinuz"))
    place_file(boot, initrd, os.path.join(dest_dir, "initramfs.img"))
    tmp = os.path.join(dest_dir, "kver.new")
    with open(tmp, "w") as f:
        f.write(kver + "\n")
    os.replace(tmp, os.path.join(dest_dir, "kver"))


def save_kernel_safe(c, boot, be, k):
    """save the kernel set of BE `be`; makes room first and NEVER raises for a full /boot (warns instead). True when saved."""
    kver, kimg, initrd = k
    need = os.path.getsize(kimg) + os.path.getsize(initrd)
    try:
        dedupe(boot)
        if not make_room(c, need, boot):
            say(f"/boot has no room for kernel {kver} ({need >> 20} MB) even after removing old automatic snapshots; "
                f"boot environment {be} keeps its previous kernel in /boot/be/{be} (free space on /boot or lower KEEP)")
            return False
        save_kernel(os.path.join(boot, "be", be), kver, kimg, initrd, boot)
        gc_objects(boot)
        return True
    except OSError as e:
        say(f"could not save kernel {kver} for {be} into /boot ({e}); keeping the previous one")
        return False


def entry_id(be, kver=None):
    """GRUB menu id: be-<be> is the BE's newest kernel; older kept kernels are be-<be>-k-<kver>"""
    return f"be-{be}" if kver is None else f"be-{be}-k-{re.sub(r'[^A-Za-z0-9_.-]', '_', kver)}"


RETRY_ID = "hx-retry"
RETRY_ROUNDS = 3


def menu_timeout(value, fallback=5):
    """boot menu countdown in seconds, always > 0: an unset, invalid, zero or negative TIMEOUT would leave the machine waiting at the
    menu (or hide it); a Horcrux machine must boot unattended"""
    try:
        t = int(str(value).strip())
    except (TypeError, ValueError):
        return fallback
    return t if t > 0 else fallback


def fallback_ids(order, kernels, previous, good, default):
    """GRUB `fallback` list for the active boot environment: its known-good kernel first, then its other kernels (newest first).
    Other boot environments are never booted automatically (that would silently roll the system back)."""
    if not order:
        return []
    a = order[0]
    ids = [entry_id(a)] + [entry_id(a, kv) for kv in (previous or {}).get(a, [])]
    gk = (good or {}).get(a)
    gid = (entry_id(a) if gk == (kernels or {}).get(a) else entry_id(a, gk)) if gk else None
    first = [gid] if gid in ids and gid != default else []
    return first + [i for i in ids if i != default and i not in first]


def render_grub(bes, active, pool, boot_uuid, cmdline="", timeout=5, serial_lines=None, title="Horcrux", kernels=None,
                previous=None, default=None, good=None, env_file="/grub/grubenv", cfg_file="/grub/grub.cfg"):
    """grub.cfg text. bes: list of BE names that have a kernel dir; active goes first and is the default.
    kernels {be: kver of the newest kernel}; previous {be: [kver, ...]} older kernels kept bootable (newest first), each its own menu
    entry; good {be: kver known to have booted}; default = menu id to boot by default (the known-good kernel while the newest one is
    still on trial). A one-shot `next_entry` in grubenv (set by `horcrux-be sync`, cleared by GRUB before it boots) wins over default.

    UNATTENDED BOOT (a machine once sat at this menu with no countdown until someone pressed Enter): GRUB turns the countdown off for
    good once an automatic boot attempt comes back to the menu (grub_menu_execute_entry unsets `timeout`: a kernel or initrd that fails
    to load, an EFI stub that returns). So the config never depends on the first attempt:
    - `fallback` = the active boot environment's other kernels (known-good first), then the retry entry, as entry numbers;
    - the retry entry reloads this file (cfg_file, relative to $root: re-arms the countdown) and after RETRY_ROUNDS rounds restarts
      the machine;
    - the environment block is read only when it exists, from the /boot file system itself (env_file, relative to $root, not $prefix);
    - a one-shot trial kernel is booted only when GRUB managed to clear next_entry first (save_env succeeded): a loader that cannot
      write the block never retries a hanging trial forever; the previous default becomes the first fallback;
    - the countdown is always > 0 (menu_timeout)."""
    timeout = menu_timeout(timeout)
    order = [active] + [b for b in bes if b != active] if active in bes else list(bes)
    ser = (serial_lines + "\nterminal_input console serial\nterminal_output console serial\n") if serial_lines else ""
    default = default or (entry_id(order[0]) if order else "")
    # GRUB's `fallback` takes menu entry NUMBERS only (get_and_remove_first_entry_number: an id there silently ends the fallback chain)
    menu_ids = [i for b in order for i in [entry_id(b)] + [entry_id(b, kv) for kv in (previous or {}).get(b, [])]] + [RETRY_ID]
    fb_ids = fallback_ids(order, kernels, previous, good, default) + [RETRY_ID]
    fb = " ".join(str(menu_ids.index(i)) for i in fb_ids)
    fb_trial = " ".join(str(menu_ids.index(i)) for i in ([default] if default in menu_ids else []) + fb_ids)
    out = [f"# generated by horcrux-be ({time.strftime('%F %T')}): do not edit, run `horcrux-be grubcfg`",
           f"set timeout={timeout}", "set timeout_style=menu", ser.rstrip("\n"),
           "insmod part_gpt", "insmod ext2", "set gfxpayload=keep", f"search --no-floppy --fs-uuid --set=root {boot_uuid}",
           f'set default="{default}"' if order else "",
           "# an automatic boot that comes back to GRUB turns its countdown off: try the other kernels, then start over (never wait for a key)",
           f'set fallback="{fb}"' if order else ""]
    if order:
        out += ["# one-shot trial of a new kernel: GRUB clears next_entry BEFORE booting it, so a kernel that hangs or panics is not tried twice.",
                "# Tried only when that clear succeeded; the block is read only when present (an error here must never stop the boot).",
                f"if [ -s {env_file} ] ; then",
                f"    load_env -f {env_file} next_entry",
                '    if [ "${next_entry}" ] ; then',
                '        set hx_trial="${next_entry}"',
                "        set next_entry=",
                f"        if save_env -f {env_file} next_entry ; then",
                f'            set fallback="{fb_trial}"',
                '            set default="${hx_trial}"',
                "        fi",
                "    fi",
                "fi"]
    out.append("")

    def entry(b, kver, newest):
        d = entry_id(b) if newest else entry_id(b, kver)
        gk = (good or {}).get(b)
        tags = []
        if d == default:
            tags.append("default")
        if gk and kver == gk:
            tags.append("known good")
        label = f"{title} - {b}" + ("" if newest else ", previous kernel") + (f" ({', '.join(tags)})" if tags else "") + (f"  [kernel {kver}]" if kver else "")
        img = f"/be/{b}/vmlinuz" if newest else f"/be/{b}/kernels/{kver}/vmlinuz"
        ird = f"/be/{b}/initramfs.img" if newest else f"/be/{b}/kernels/{kver}/initramfs.img"
        return [f'menuentry "{label}" --id {d} {{', f"    linux {img} root=ZFS={pool}/ROOT/{b} zfsforce=1 ro {cmdline}".rstrip(),
                f"    initrd {ird}", "}"]

    for b in order:
        out += entry(b, (kernels or {}).get(b, ""), True)
        for kv in (previous or {}).get(b, []):
            out += entry(b, kv, False)
    if order:
        # reached through `fallback` once every kernel above failed to start (or by hand): reloading the file re-arms the countdown;
        # after RETRY_ROUNDS rounds the machine restarts, so it never waits at a menu for a key
        out += [f'menuentry "Start the boot menu again (automatic retry)" --id {RETRY_ID} {{',
                f'    if [ "${{hx_tries}}" = "{"x" * RETRY_ROUNDS}" ] ; then',
                '        echo "No kernel could be started: restarting the machine in 5 seconds."',
                "        sleep 5",
                "        reboot",
                "    fi",
                '    set hx_tries="x${hx_tries}"',
                "    export hx_tries",
                # an explicit path on the /boot file system: GRUB restores ${config_file} once the top-level config is read (empty here)
                f"    configfile {cfg_file}",
                "}"]
    return "\n".join(l for l in out if l is not None) + "\n"


def boot_uuid(boot=BOOT):
    dev = sh(["findmnt", "-no", "SOURCE", boot], check=False).split("[")[0]
    return sh(["blkid", "-s", "UUID", "-o", "value", dev], check=False) if dev else ""


def be_kernel_dirs(boot, be):
    """{kver: dir} of the kernels saved for `be`: the newest (be/<be>) plus the kept previous ones (be/<be>/kernels/<kver>)"""
    res = {}
    top = os.path.join(boot, "be", be)
    for d in [top] + sorted(glob.glob(os.path.join(top, "kernels", "*"))):
        try:
            kv = open(os.path.join(d, "kver")).read().strip()
        except OSError:
            continue
        if kv and os.path.isfile(os.path.join(d, "vmlinuz")) and os.path.isfile(os.path.join(d, "initramfs.img")):
            res[kv] = d
    return res


def be_kernels(boot, be):
    """(newest kver, [older kvers newest first]) saved for `be`; the top-level dir IS the newest by definition"""
    top = ""
    try:
        top = open(os.path.join(boot, "be", be, "kver")).read().strip()
    except OSError:
        pass
    rest = sorted((k for k in be_kernel_dirs(boot, be) if k != top), key=vkey, reverse=True)
    return top, rest


def good_kernel(boot, be):
    try:
        return open(os.path.join(boot, "be", be, "good")).read().strip()
    except OSError:
        return ""


def write_text(path, text):
    tmp = path + ".new"
    with open(tmp, "w") as f:
        f.write(text)
    os.replace(tmp, path)


def kernel_keep(c):
    try:
        return max(1, int(c.get("KERNELS_KEEP", "2")))
    except ValueError:
        return 2


def rotate_current(boot, be, new_kver):
    """the BE's current kernel is about to be replaced by `new_kver`: keep it as a previous kernel (hard links, no extra space)"""
    top = os.path.join(boot, "be", be)
    try:
        old = open(os.path.join(top, "kver")).read().strip()
    except OSError:
        return
    if not old or old == new_kver or not os.path.isfile(os.path.join(top, "vmlinuz")) or not os.path.isfile(os.path.join(top, "initramfs.img")):
        return
    dest = os.path.join(top, "kernels", old)
    copy_tree(boot, top, dest)
    write_text(os.path.join(dest, "kver"), old + "\n")


def retain_kernels(c, boot, be, seed_loose=False):
    """Keep the newest KERNELS_KEEP kernels of `be` bootable (the known-good one always stays, even when older), drop the rest.
    seed_loose: the BE is the running system, so kernels installkernel left in /boot (whose /lib/modules still exist) are saved too:
    that is how the kernel from BEFORE this tool started keeping history becomes the 'previous kernel' entry."""
    have = be_kernel_dirs(boot, be)
    top, _ = be_kernels(boot, be)
    good = good_kernel(boot, be)
    if seed_loose:
        mods = c.get("MODULES_DIR", "/lib/modules")
        loose = {kv: (k, ird) for kv, k, ird in loose_kernels(boot, c.get("KERNEL_GLOB", "*")) if os.path.isdir(os.path.join(mods, kv))}
        wanted = set(sorted(set(have) | set(loose), key=vkey, reverse=True)[:kernel_keep(c)])
        if good in loose:
            wanted.add(good)
        for kv in sorted(wanted - set(have), key=vkey, reverse=True):
            k, ird = loose[kv]
            need = os.path.getsize(k) + os.path.getsize(ird)
            if not space_ok(c, need, boot):
                say(f"/boot has no room to keep kernel {kv} as the previous kernel ({need >> 20} MB); lower KEEP or free /boot")
                continue
            try:
                dest = os.path.join(boot, "be", be, "kernels", kv)
                save_kernel(dest, kv, k, ird, boot=boot)
                have[kv] = dest
            except OSError as e:
                say(f"could not keep kernel {kv} ({e})")
    keep = set(sorted(have, key=vkey, reverse=True)[:kernel_keep(c)])
    if top in have:
        keep.add(top)  # the top-level dir is what the BE boots (and clones restore): it is never removed here
    if good in have:
        keep.add(good)
    for kv, d in have.items():
        if kv not in keep and d != os.path.join(boot, "be", be):
            shutil.rmtree(d, ignore_errors=True)
    gc_objects(boot)


def grubenv_path(c, boot=BOOT):
    return c.get("GRUBENV") or os.path.join(boot, "grub", "grubenv")


def set_next_entry(c, boot, entry):
    """one-shot boot of `entry` (empty = clear). False when GRUB's environment block cannot be written"""
    if not shutil.which("grub-editenv"):
        return False
    env = grubenv_path(c, boot)
    if not entry and not os.path.exists(env):
        return True  # nothing to clear
    try:
        os.makedirs(os.path.dirname(env), exist_ok=True)
        if not os.path.exists(env):
            subprocess.run(["grub-editenv", env, "create"], check=True, capture_output=True)
        subprocess.run(["grub-editenv", env, "set" if entry else "unset", *([f"next_entry={entry}"] if entry else ["next_entry"])],
                       check=True, capture_output=True)
        return True
    except (OSError, subprocess.CalledProcessError):
        return False


def ensure_grubenv(c, boot=BOOT):
    """the grub.cfg reads GRUB's environment block (load_env): make sure the file exists so GRUB never prints a 'file not found' error at the menu"""
    env = grubenv_path(c, boot)
    if os.path.exists(env) or not shutil.which("grub-editenv"):
        return
    try:
        os.makedirs(os.path.dirname(env), exist_ok=True)
        subprocess.run(["grub-editenv", env, "create"], check=True, capture_output=True)
    except (OSError, subprocess.CalledProcessError):
        pass


def good_service_enabled():
    """is the boot-time `horcrux-be mark-good` service enabled (OpenRC or systemd)? Without it a trial kernel would never be promoted"""
    try:
        if os.path.isdir("/run/systemd/system"):
            return subprocess.run(["systemctl", "is-enabled", "-q", "horcrux-be-good.service"], capture_output=True).returncode == 0
        return subprocess.run("rc-update show -v | grep -q '^ *horcrux-be-good '", shell=True, capture_output=True).returncode == 0
    except OSError:
        return False


def trial_plan(c, boot, be):
    """(default menu id, one-shot menu id or None, good kver). While the newest kernel has not booted successfully yet and an older
    kernel did, the older one stays the default and the newest is tried once; with no known-good older kernel (first install, trial off,
    mark-good service missing, loader without next_entry support) the newest is simply the default."""
    top, rest = be_kernels(boot, be)
    good = good_kernel(boot, be)
    newest = entry_id(be)
    if (c.get("KERNEL_TRIAL", "yes") == "yes" and c.get("BOOT_LOADER", "grub") == "grub" and top and good and good != top and good in rest
            and good_service_enabled()):
        return entry_id(be, good), newest, good
    return newest, None, good


def grubcfg(c, boot=BOOT, uuid=None):
    pool = c["POOL"]
    bes = [b for b in be_datasets(pool) if os.path.isfile(os.path.join(boot, "be", b, "vmlinuz"))]
    act = active_be(pool) or (bes[0] if bes else None)
    kern, prev, goods = {}, {}, {}
    for b in bes:
        kern[b], prev[b] = be_kernels(boot, b)
        goods[b] = good_kernel(boot, b)
    default = trial_plan(c, boot, act)[0] if act in bes else None
    serial = None
    try:
        from . import arch
        serial = arch.grub_serial_lines()
    except Exception:  # noqa
        pass
    env = os.path.relpath(grubenv_path(c, boot), boot)
    env = "/" + env if not env.startswith("..") else "/grub/grubenv"   # GRUB reads it from the /boot file system ($root)
    text = render_grub(bes, act, pool, uuid or boot_uuid(boot), c.get("CMDLINE", ""), c.get("TIMEOUT", "5"), serial, kernels=kern,
                       previous=prev, default=default, good=goods, env_file=env)
    os.makedirs(os.path.join(boot, "grub"), exist_ok=True)
    ensure_grubenv(c, boot)
    tmp = os.path.join(boot, "grub", "grub.cfg.new")
    open(tmp, "w").write(text)
    os.replace(tmp, os.path.join(boot, "grub", "grub.cfg"))
    if boot == BOOT and os.path.exists("/etc/horcrux/boot-mirrors.conf"):
        # multi-disk pool: the other drives' /boot copies must follow the real grub.cfg (best effort)
        subprocess.run(["horcrux-boot-sync"], check=False)
    return text


def cmd_sync(c, boot=BOOT):
    """save the newest installed kernel as /boot/be/<running BE>/ (keeping the previous ones bootable) and refresh grub.cfg"""
    be = running_be() or active_be(c["POOL"]) or "horcrux"
    k = kernel_files(boot, c.get("KERNEL_GLOB", "*"))
    if not k:
        raise SystemExit("horcrux-be: no kernel + initramfs pair in /boot")
    # a machine that never ran mark-good: the kernel it is running now is the known-good one (that is how an existing install migrates)
    if not good_kernel(boot, be):
        run = running_kernel()
        if run in be_kernel_dirs(boot, be) or run == k[0] or os.path.exists(os.path.join(boot, f"vmlinuz-{run}")):
            seed_good = run
        else:
            seed_good = ""
    else:
        seed_good = ""
    try:
        rotate_current(boot, be, k[0])
    except OSError as e:
        say(f"could not keep the current kernel as the previous one ({e})")
    top = os.path.join(boot, "be", be)
    try:  # a re-sync of the kernel the BE already holds needs no room and must not warn about a full /boot
        same = (be_kernels(boot, be)[0] == k[0] and os.path.getsize(os.path.join(top, "vmlinuz")) == os.path.getsize(k[1])
                and os.path.getsize(os.path.join(top, "initramfs.img")) == os.path.getsize(k[2]))
    except OSError:
        same = False
    ok = True if same else save_kernel_safe(c, boot, be, k)
    try:
        retain_kernels(c, boot, be, seed_loose=True)
    except OSError as e:
        say(f"could not keep previous kernels ({e})")
    if seed_good and seed_good in be_kernel_dirs(boot, be):
        write_text(os.path.join(boot, "be", be, "good"), seed_good + "\n")
    default, once, good = trial_plan(c, boot, be)
    grubcfg(c, boot)
    if c.get("BOOT_LOADER", "grub") == "grub":
        if once:
            if set_next_entry(c, boot, once):
                say(f"kernel {k[0]} boots ONCE as a trial; {good} stays the default until it has booted successfully (mark-good)")
            else:
                say("grub-editenv unavailable: the new kernel is not trial-booted; the previous kernel entry stays in the menu")
        else:
            set_next_entry(c, boot, "")
    print(f"kernel {k[0]} -> /boot/be/{be}" if ok else f"kernel {k[0]} NOT copied to /boot/be/{be} (see above)")


def cmd_mark_good(c, boot=BOOT, kver=None, wait=0):
    """the running kernel booted successfully: make it the known-good kernel and the default (called at the end of every boot)"""
    be = running_be() or active_be(c["POOL"])
    if not be:
        return
    kver = kver or running_kernel()
    if wait:
        time.sleep(wait)
    pool = c["POOL"]
    st = subprocess.run(["zpool", "list", "-H", "-o", "health", pool], capture_output=True, text=True)
    if st.returncode != 0 or st.stdout.strip() not in ("ONLINE", "DEGRADED"):
        say(f"pool {pool} is not healthy ({st.stdout.strip() or st.stderr.strip()}): kernel {kver} NOT marked known-good")
        return
    if kver not in be_kernel_dirs(boot, be):
        say(f"kernel {kver} is not one saved for {be}: nothing to mark")
        return
    if good_kernel(boot, be) == kver:
        return
    write_text(os.path.join(boot, "be", be, "good"), kver + "\n")
    if be == (active_be(pool) or be):
        grubcfg(c, boot)
        set_next_entry(c, boot, "")
    say(f"kernel {kver} marked known-good for {be}")


def cmd_boot_previous(c, boot=BOOT):
    """next boot only: the previous kernel of the running BE (the one-key rollback from a shell); nothing is rebooted"""
    be = running_be() or active_be(c["POOL"])
    top, rest = be_kernels(boot, be)
    if not rest:
        raise SystemExit(f"horcrux-be: {be} keeps no previous kernel (KERNELS_KEEP={kernel_keep(c)})")
    pick = good_kernel(boot, be) if good_kernel(boot, be) in rest else rest[0]
    if not set_next_entry(c, boot, entry_id(be, pick)):
        raise SystemExit("horcrux-be: cannot write GRUB's environment block (grub-editenv)")
    print(f"the next boot (only) uses kernel {pick}")


def cmd_kernels(c, boot=BOOT, as_json=False):
    out = []
    for b in be_datasets(c["POOL"]):
        top, rest = be_kernels(boot, b)
        g = good_kernel(boot, b)
        for kv in ([top] if top else []) + rest:
            out.append({"be": b, "kernel": kv, "newest": kv == top, "good": kv == g, "running": b == running_be() and kv == running_kernel()})
    if as_json:
        import json
        print(json.dumps(out))
    else:
        print(f"{'BE':<16} {'KERNEL':<34} FLAGS")
        for e in out:
            print(f"{e['be']:<16} {e['kernel']:<34} {' '.join(n for n in ('newest', 'known-good', 'running') if e[{'known-good': 'good'}.get(n, n)])}")


def cmd_snapshot(c, name=None, boot=BOOT):
    pool = c["POOL"]
    be = running_be() or active_be(pool)
    if not be:
        raise SystemExit("horcrux-be: no active boot environment")
    name = name or time.strftime("manual-%Y%m%d-%H%M%S")
    if not valid_name(name):
        raise SystemExit("horcrux-be: bad snapshot name")
    src = os.path.join(boot, "be", be)
    if os.path.isdir(src):
        try:
            dedupe(boot)  # old plain copies -> links, so the new copy below costs nothing
            # the copy is hard links to objects that already exist: it needs no space, but keep /boot under the limit anyway
            make_room(c, 0, boot)
        except OSError as e:
            say(f"could not tidy /boot ({e}); continuing")
    sh(["zfs", "snapshot", f"{pool}/ROOT/{be}@{name}"])
    if os.path.isdir(src):
        dest = snap_dir(boot, be, name)
        try:
            copy_tree(boot, src, dest)
        except OSError as e:  # a full /boot never fails the snapshot (or the upgrade behind it): the ZFS snapshot stands, the kernel copy is skipped
            shutil.rmtree(dest, ignore_errors=True)
            say(f"snapshot {be}@{name} taken, but its kernel could not be saved in /boot ({e}); "
                f"a rollback to it will reuse the current kernel")
    prune(c, boot)
    print(f"snapshot {pool}/ROOT/{be}@{name}")
    return f"{be}@{name}"


def cmd_auto(c, boot=BOOT):
    """pre-upgrade snapshot used by the portage hook: at most one per 30 minutes, never fatal"""
    if c.get("AUTO_SNAPSHOT") != "yes" or not shutil.which("zfs"):
        return
    stamp = "/run/horcrux-be-auto"
    try:
        if time.time() - os.path.getmtime(stamp) < 1800:
            return
    except OSError:
        pass
    try:
        if not active_be(c["POOL"]) and not running_be():
            return
        cmd_snapshot(c, time.strftime("pre-upgrade-%Y%m%d-%H%M%S"), boot)
        open(stamp, "w").close()
    except (SystemExit, OSError) as e:
        print(f"horcrux-be: auto snapshot skipped: {e}", file=sys.stderr)


def snapshots(pool):
    """[(be, snapshot, creation)] oldest first; None when ZFS could not be asked (callers must not treat that as 'no snapshots')"""
    r = subprocess.run(["zfs", "list", "-H", "-t", "snapshot", "-o", "name,creation", "-s", "creation", "-r", f"{pool}/ROOT"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        return None
    res = []
    for l in r.stdout.strip().splitlines():
        n, _, cr = l.partition("\t")
        m = re.fullmatch(rf"{re.escape(pool)}/ROOT/([^@/]+)@(.+)", n)
        if m:
            res.append((m.group(1), m.group(2), cr))
    return res


def prune(c, boot=BOOT):
    """KEEP newest automatic snapshots (pre-upgrade-*, create-* nobody cloned); then drop /boot copies without a snapshot and objects
    nobody links to; then bring /boot back under the usage limit"""
    keep = int(c.get("KEEP", "5"))
    sn = snapshots(c["POOL"])
    if sn is None:
        return
    org = origins(c["POOL"]) | pinned(c["POOL"])  # pinned snapshots never count against KEEP and are never pruned
    for prefix in ("pre-upgrade-", "create-"):
        auto = [s for s in sn if s[1].startswith(prefix) and AUTO_SNAP.fullmatch(s[1]) and f"{s[0]}@{s[1]}" not in org]
        for be, snap, _cr in auto[:-keep] if keep > 0 else auto:
            destroy_snapshot(c, be, snap, boot)
    sn = snapshots(c["POOL"])
    if sn is not None:  # saved kernels whose snapshot is gone
        live = {f"{b}@{s}" for b, s, _ in sn}
        for d in glob.glob(os.path.join(boot, "be", ".snap", "*")):
            if os.path.basename(d) not in live:
                shutil.rmtree(d, ignore_errors=True)
    gc_objects(boot)
    make_room(c, 0, boot)


def cmd_create(c, name, from_snap=None, boot=BOOT):
    pool = c["POOL"]
    if not valid_name(name):
        raise SystemExit("horcrux-be: bad boot environment name")
    if name in be_datasets(pool):
        raise SystemExit(f"horcrux-be: {name} already exists")
    be = running_be() or active_be(pool)
    if not be:
        raise SystemExit("horcrux-be: no active boot environment")
    if from_snap:
        src_be, _, snap = from_snap.rpartition("@") if "@" in from_snap else (be, "", from_snap)
        src_be = src_be or be
    else:
        snap = time.strftime("create-%Y%m%d-%H%M%S")
        cmd_snapshot(c, snap, boot)
        src_be = be
    sh(["zfs", "clone", "-o", "canmount=noauto", "-o", "mountpoint=/", f"{pool}/ROOT/{src_be}@{snap}", f"{pool}/ROOT/{name}"])
    saved = snap_dir(boot, src_be, snap)
    if os.path.isdir(saved):
        copy_tree(boot, saved, os.path.join(boot, "be", name))
    else:
        print("horcrux-be: warning: no kernel was saved with this snapshot; using the current kernel (modules may not match)", file=sys.stderr)
        copy_tree(boot, os.path.join(boot, "be", be), os.path.join(boot, "be", name))
    grubcfg(c, boot)
    print(f"created {pool}/ROOT/{name} from {src_be}@{snap}")


def cmd_activate(c, name, boot=BOOT):
    pool = c["POOL"]
    if name not in be_datasets(pool):
        raise SystemExit(f"horcrux-be: no boot environment {name}")
    if not os.path.isfile(os.path.join(boot, "be", name, "vmlinuz")):
        raise SystemExit(f"horcrux-be: {name} has no kernel in /boot/be/{name}")
    sh(["zpool", "set", f"bootfs={pool}/ROOT/{name}", pool])
    grubcfg(c, boot)
    print(f"{name} will boot next (default in the GRUB menu)")


def cmd_destroy(c, name, boot=BOOT):
    pool = c["POOL"]
    if name in (running_be(), active_be(pool)):
        raise SystemExit("horcrux-be: refusing to destroy the running/active boot environment")
    if name in pinned(pool):
        raise SystemExit(f"horcrux-be: {name} is pinned (horcrux-be unpin {name} first)")
    sh(["zfs", "destroy", "-r", f"{pool}/ROOT/{name}"])
    shutil.rmtree(os.path.join(boot, "be", name), ignore_errors=True)
    gc_objects(boot)
    grubcfg(c, boot)


def cmd_list(c):
    pool = c["POOL"]
    act, run = active_be(pool), running_be()
    print(f"{'BE':<20} {'ACTIVE':<7} {'RUNNING':<8} KERNEL")
    for b in be_datasets(pool):
        kv = ""
        try:
            kv = open(os.path.join(BOOT, "be", b, "kver")).read().strip()
        except OSError:
            pass
        print(f"{b:<20} {'yes' if b == act else '':<7} {'yes' if b == run else '':<8} {kv}")
    sn = snapshots(pool) or []
    total, free = boot_space()
    print(f"\n/boot: {100 - 100 * free // max(total, 1)}% used, {free >> 20} MB free")
    if sn:
        print("\nsnapshots:")
        for be, s, cr in sn:
            print(f"  {be}@{s}  {cr}")


def info(c):
    """everything a UI needs, as a dict (horcrux-be list --json)"""
    pool = c["POOL"]
    act, run = active_be(pool), running_be()
    pins = pinned(pool)
    org = {}
    out = sh(["zfs", "list", "-H", "-p", "-o", "name,origin,creation,used", "-r", "-d", "1", f"{pool}/ROOT"], check=False)
    envs = []
    for l in out.splitlines():
        f = l.split("\t")
        if len(f) < 4 or f[0].count("/") != 2:
            continue
        b = f[0].split("/")[-1]
        try:
            kv = open(os.path.join(BOOT, "be", b, "kver")).read().strip()
        except OSError:
            kv = ""
        envs.append({"name": b, "active": b == act, "running": b == run, "kernel": kv, "pinned": b in pins,
                     "origin": f[1].split("/")[-1] if f[1] != "-" else None, "created": int(f[2]), "used": int(f[3]),
                     "previous_kernels": be_kernels(BOOT, b)[1], "good_kernel": good_kernel(BOOT, b)})
    snaps = []
    out = sh(["zfs", "list", "-H", "-p", "-t", "snapshot", "-o", "name,creation,used", "-s", "creation", "-r", f"{pool}/ROOT"], check=False)
    for l in out.splitlines():
        f = l.split("\t")
        m = re.fullmatch(rf"{re.escape(pool)}/ROOT/([^@/]+)@(.+)", f[0])
        if m and len(f) >= 3:
            sd = snap_dir(BOOT, m.group(1), m.group(2))
            try:
                kv = open(os.path.join(sd, "kver")).read().strip()
            except OSError:
                kv = ""
            snaps.append({"be": m.group(1), "name": m.group(2), "created": int(f[1]), "used": int(f[2]), "kernel": kv,
                          "pinned": f"{m.group(1)}@{m.group(2)}" in pins, "auto": bool(AUTO_SNAP.fullmatch(m.group(2)))})
    total, free = boot_space()
    return {"pool": pool, "running": run, "active": act, "keep": int(c.get("KEEP", "5")), "auto_snapshot": c.get("AUTO_SNAPSHOT") == "yes",
            "environments": envs, "snapshots": snaps, "boot": {"total": total, "free": free}, "data": data_isolation(c),
            "preserved": [l for l in sh(["zfs", "list", "-H", "-o", "name", "-r", pool], check=False).splitlines()
                          if l != pool and not (l == f"{pool}/ROOT" or l.startswith(f"{pool}/ROOT/"))]}


def previous_snapshot(c):
    """the newest automatic (pre-upgrade/create) snapshot of the running BE: what `rollback` with no argument returns to"""
    be = running_be() or active_be(c["POOL"])
    auto = [f"{b}@{s}" for b, s, _ in (snapshots(c["POOL"]) or []) if b == be and AUTO_SNAP.fullmatch(s)]
    if not auto:
        raise SystemExit(f"horcrux-be: {be} has no pre-upgrade snapshot to return to (see `horcrux-be list`)")
    return auto[-1]


def init_installer(c, be="horcrux", boot=BOOT, uuid=None):
    """called by the installer in the chroot: first BE kernel dir + grub.cfg"""
    k = kernel_files(boot, c.get("KERNEL_GLOB", "*"))
    if not k:
        raise SystemExit("horcrux-be: no kernel + initramfs pair in /boot")
    save_kernel(os.path.join(boot, "be", be), *k, boot=boot)
    write_text(os.path.join(boot, "be", be, "good"), k[0] + "\n")  # the installer's own kernel is the baseline: nothing older to fall back to
    sh(["zpool", "set", f"bootfs={c['POOL']}/ROOT/{be}", c["POOL"]], check=False)
    return grubcfg(c, boot, uuid)


def main(argv=None):
    ap = argparse.ArgumentParser(prog="horcrux-be", description="ZFS boot environments and rollback")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list"); p.add_argument("--json", action="store_true")
    p = sub.add_parser("pin", help="keep a boot environment or BE@snapshot forever (never pruned)"); p.add_argument("name")
    p = sub.add_parser("unpin"); p.add_argument("name")
    sub.add_parser("check", help="verify that user data / VM disks are outside the boot environments (exit 1 when not)")
    p = sub.add_parser("snapshot"); p.add_argument("name", nargs="?")
    p = sub.add_parser("create"); p.add_argument("name"); p.add_argument("--from", dest="src", help="snapshot, BE@snap")
    p = sub.add_parser("activate"); p.add_argument("name"); p.add_argument("--force", action="store_true")
    p = sub.add_parser("destroy"); p.add_argument("name")
    p = sub.add_parser("rollback", help="create a BE from a snapshot and activate it (then reboot)")
    p.add_argument("snapshot", nargs="?", help="BE@snapshot; default: the newest pre-upgrade snapshot (boot the previous version)")
    p.add_argument("--name"); p.add_argument("--force", action="store_true")
    p = sub.add_parser("destroy-snapshot", help="destroy a snapshot and its /boot copy"); p.add_argument("snapshot", help="BE@snap")
    sub.add_parser("dedupe", help="share identical kernel copies under /boot/be (hard links) and drop orphans")
    p = sub.add_parser("mark-good", help="the running kernel booted fine: make it the known-good kernel/default (run at the end of every boot)")
    p.add_argument("--kver"); p.add_argument("--wait", action="store_true", help="first wait GOOD_DELAY seconds (boot-time service)")
    sub.add_parser("boot-previous", help="boot the previous kernel on the NEXT boot only (does not reboot)")
    p = sub.add_parser("kernels", help="kernels kept bootable per boot environment"); p.add_argument("--json", action="store_true")
    sub.add_parser("sync"); sub.add_parser("grubcfg"); sub.add_parser("auto-snapshot"); sub.add_parser("prune")
    p = sub.add_parser("init"); p.add_argument("--be", default="horcrux"); p.add_argument("--boot-uuid")
    a = ap.parse_args(argv)
    if os.geteuid() != 0:
        raise SystemExit("horcrux-be: run as root")
    c = conf()
    if a.cmd == "list":
        if a.json:
            import json
            print(json.dumps(info(c)))
        else:
            cmd_list(c)
    elif a.cmd == "pin":
        cmd_pin(c, a.name)
    elif a.cmd == "unpin":
        cmd_pin(c, a.name, False)
    elif a.cmd == "check":
        d = data_isolation(c)
        for x in d:
            print(f"{'ok  ' if x['safe'] else 'FAIL'} {x['path']:<26} {x['dataset'] or '-':<32} {x['why']}")
        return 0 if all(x["safe"] for x in d) else 1
    elif a.cmd == "snapshot":
        cmd_snapshot(c, a.name)
    elif a.cmd == "create":
        cmd_create(c, a.name, a.src)
    elif a.cmd == "activate":
        isolation_guard(c, a.force)
        cmd_activate(c, a.name)
    elif a.cmd == "destroy":
        cmd_destroy(c, a.name)
    elif a.cmd == "rollback":
        isolation_guard(c, a.force)
        snap = a.snapshot or previous_snapshot(c)
        nm = a.name or "rollback-" + time.strftime("%Y%m%d-%H%M%S")
        cmd_create(c, nm, snap)
        cmd_activate(c, nm)
        print("reboot to enter it; the previous boot environment stays in the boot menu")
    elif a.cmd == "destroy-snapshot":
        be, _, sn = a.snapshot.partition("@")
        if f"{be}@{sn}" in pinned(c["POOL"]):
            raise SystemExit(f"horcrux-be: {a.snapshot} is pinned (horcrux-be unpin {a.snapshot} first)")
        if not sn or not destroy_snapshot(c, be, sn):
            raise SystemExit(f"horcrux-be: could not destroy {a.snapshot}")
    elif a.cmd == "dedupe":
        print(f"saved {dedupe() >> 20} MB")
        prune(c)
    elif a.cmd == "sync":
        cmd_sync(c)
    elif a.cmd == "mark-good":
        cmd_mark_good(c, kver=a.kver, wait=int(c.get("GOOD_DELAY", "60")) if a.wait else 0)
    elif a.cmd == "boot-previous":
        cmd_boot_previous(c)
    elif a.cmd == "kernels":
        cmd_kernels(c, as_json=a.json)
    elif a.cmd == "grubcfg":
        grubcfg(c)
    elif a.cmd == "auto-snapshot":
        cmd_auto(c)
    elif a.cmd == "prune":
        prune(c)
    elif a.cmd == "init":
        init_installer(c, a.be, uuid=a.boot_uuid)
    return 0
