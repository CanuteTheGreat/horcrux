"""`horcrux-be migrate-data`: move data directories that still live INSIDE the boot environment's own dataset (installs from before
the installer gave /var/lib/horcrux/vms its own dataset) onto datasets of their own, so a rollback or boot-previous never hides them.

For every path of DATA_PATHS that is a plain directory on `<pool>/ROOT/<be>` and holds files:
  1. refuse while anything has files open under it (a running VM, docker, libvirt): the data must be quiet, stop those services first
  2. snapshot the boot environment (`migrate-data-<stamp>`, never pruned): the original data stays reachable there
  3. create `<pool>/<path>` (parents as canmount=off containers), copy into it (rsync -aHAXS, else cp -a), verify every file
  4. swap: move the old directory aside, mount the dataset at the path, remove the old copy (--keep-original keeps it)
Idempotent (a migrated path is on its own dataset and skipped; an interrupted run is resumed, its half-copied dataset reused) and
`--dry-run` prints the plan and every blocker without changing anything. Exit 0 = nothing (left) to do or done, 3 = blocked.
Pure planning (`plan`) is unit-tested with fakes for zfs/findmnt/proc; the QEMU test migrates a real old-layout install.
"""
import json
import os
import re
import shutil
import subprocess
import time

MIGRATING_PROP = "horcrux:migrating"
TMPROOT = "/run/horcrux-migrate"
RECORDSIZE = {"/var/lib/horcrux/vms": "64K", "/var/lib/libvirt": "64K"}  # qcow2 clusters are 64 KiB


def dataset_for(pool, path):
    return f"{pool}/{path.strip('/')}"


def findmnt_source(path, run=subprocess.run):
    r = run(["findmnt", "-no", "SOURCE", "-T", path], capture_output=True, text=True)
    return r.stdout.strip().splitlines()[0] if r.stdout.strip() else ""


def open_files(path, proc="/proc"):
    """[(pid, comm)] of processes with a file descriptor, working directory or root below `path`"""
    base, found = os.path.realpath(path), {}
    try:
        pids = [p for p in os.listdir(proc) if p.isdigit()]
    except OSError:
        return []
    for pid in pids:
        try:
            links = [os.readlink(f"{proc}/{pid}/cwd")] + [os.readlink(f"{proc}/{pid}/fd/{fd}") for fd in os.listdir(f"{proc}/{pid}/fd")]
        except OSError:
            continue
        if any(l == base or l.startswith(base + "/") for l in links):
            try:
                found[pid] = open(f"{proc}/{pid}/comm").read().strip()
            except OSError:
                found[pid] = "?"
    return sorted(found.items(), key=lambda x: int(x[0]))


def tree_size(path):
    total = 0
    for dp, _d, fns in os.walk(path):
        for n in fns:
            try:
                total += os.lstat(os.path.join(dp, n)).st_size
            except OSError:
                pass
    return total


def pool_avail(pool, run=subprocess.run):
    r = run(["zfs", "list", "-Hp", "-o", "avail", pool], capture_output=True, text=True)
    try:
        return int(r.stdout.split()[0])
    except (IndexError, ValueError):
        return 0


def dataset_props(ds, run=subprocess.run):
    r = run(["zfs", "get", "-H", "-o", "property,value", f"{MIGRATING_PROP},mounted,mountpoint", ds], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    return dict(l.split("\t", 1) for l in r.stdout.splitlines() if "\t" in l)


def plan(c, paths=None, run=subprocess.run, opens=None, size=None, avail=None):
    """[{path, dataset, action, bytes, blockers}] for every path that needs (or resumes) a migration. action: migrate|resume|skip,
    `why` says why a path is skipped. Pure apart from the injected probes."""
    opens, size, avail = opens or open_files, size or tree_size, avail or pool_avail
    pool, res = c["POOL"], []
    for path in paths or c.get("DATA_PATHS", "").split():
        ds = dataset_for(pool, path)
        row = {"path": path, "dataset": ds, "action": "skip", "why": "", "bytes": 0, "blockers": []}
        res.append(row)
        if not os.path.isdir(path):
            row["why"] = "absent"
            continue
        src = findmnt_source(path, run)
        if not src.startswith(f"{pool}/ROOT/"):
            row["why"] = "already on its own dataset" if src.startswith(pool + "/") else f"not on the ZFS boot environment ({src or 'unknown source'})"
            continue
        if not os.listdir(path):
            row["why"] = "empty (nothing to move)"
            continue
        have = dataset_props(ds, run)
        if have is not None and have.get(MIGRATING_PROP, "-") not in ("yes",):
            row["action"], row["why"] = "skip", f"{ds} already exists and is not a half-finished migration: not touching it"
            row["blockers"].append(row["why"])
            continue
        row["action"] = "resume" if have is not None else "migrate"
        row["bytes"] = size(path)
        if row["bytes"] and avail(pool, run) < row["bytes"] * 1.05 + (64 << 20):
            row["blockers"].append(f"not enough free space in {pool} for a second copy ({row['bytes'] >> 20} MB needed)")
        for pid, comm in opens(path):
            row["blockers"].append(f"in use by pid {pid} ({comm}): stop it first")
    return res


def _sh(cmd, run=subprocess.run):
    r = run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {(r.stderr or r.stdout).strip()}")
    return r.stdout


def make_parents(pool, ds, run=subprocess.run):
    """container datasets (canmount=off) for every missing parent below the pool: `zfs create -p` would make them mountable and they
    would mount over /var/lib/horcrux (the config database) and hide it"""
    parts = ds.split("/")[1:-1]
    for i in range(len(parts)):
        p = "/".join([pool] + parts[:i + 1])
        if run(["zfs", "list", "-H", "-o", "name", p], capture_output=True, text=True).returncode != 0:
            _sh(["zfs", "create", "-o", "canmount=off", p], run)


def verify_copy(src, dst):
    """[(relpath, problem)] differences between two trees (type, size, mode, owner, mtime to the second, symlink target)"""
    bad = []

    def entries(root):
        out = {}
        for dp, dn, fns in os.walk(root):
            for n in dn + fns:
                p = os.path.join(dp, n)
                st = os.lstat(p)
                out[os.path.relpath(p, root)] = (st.st_mode, 0 if os.path.isdir(p) and not os.path.islink(p) else st.st_size, st.st_uid, st.st_gid,
                                                 int(st.st_mtime) if not os.path.isdir(p) else 0, os.readlink(p) if os.path.islink(p) else "")
        return out

    a, b = entries(src), entries(dst)
    for k in sorted(set(a) | set(b)):
        if k not in b:
            bad.append((k, "missing in the copy"))
        elif k not in a:
            bad.append((k, "extra in the copy"))
        elif a[k] != b[k]:
            bad.append((k, "differs"))
    return bad


def copy_tree(src, dst, run=subprocess.run):
    if shutil.which("rsync"):
        _sh(["rsync", "-aHAXS", "--numeric-ids", src.rstrip("/") + "/", dst.rstrip("/") + "/"], run)
    else:
        _sh(["cp", "-a", "--sparse=always", src.rstrip("/") + "/.", dst.rstrip("/") + "/"], run)


def migrate_one(c, row, keep_original=False, run=subprocess.run, say=print, snapshot=None):
    """the four steps for one plan row (see module docstring). Raises RuntimeError with the reason; the original data is never lost:
    on any failure before the swap nothing visible changed, on a failed mount the old directory is put back."""
    path, ds, pool = row["path"], row["dataset"], c["POOL"]
    tmp = f"{TMPROOT}/{ds.replace('/', '_')}"
    if row["action"] == "migrate":
        make_parents(pool, ds, run)
        args = ["zfs", "create", "-o", "canmount=noauto", "-o", f"mountpoint={tmp}", "-o", f"{MIGRATING_PROP}=yes"]
        if path in RECORDSIZE:
            args += ["-o", f"recordsize={RECORDSIZE[path]}"]
        _sh(args + [ds], run)
    else:
        _sh(["zfs", "set", f"mountpoint={tmp}", ds], run)
    os.makedirs(tmp, exist_ok=True)
    if run(["findmnt", "-n", tmp], capture_output=True, text=True).returncode != 0:
        _sh(["zfs", "mount", ds], run)
    say(f"  copying {path} -> {ds} ({row['bytes'] >> 20} MB)")
    copy_tree(path, tmp, run)
    bad = verify_copy(path, tmp)
    if bad:
        raise RuntimeError(f"copy of {path} does not match the original ({len(bad)} differences, first: {bad[0][0]}: {bad[0][1]}); the original is untouched")
    # swap: files written between the copy and now would be missed, so the paths were checked quiet and are copied once more (rsync no-op)
    if open_files(path):
        raise RuntimeError(f"{path} was opened by a process during the copy; stop it and run migrate-data again")
    copy_tree(path, tmp, run)
    stash = f"{path}.hx-migrating"
    if os.path.exists(stash):
        raise RuntimeError(f"{stash} exists from an earlier attempt: look at it, move it away, run migrate-data again")
    run(["zfs", "unmount", ds], capture_output=True, text=True)
    os.rename(path, stash)
    os.makedirs(path)
    try:
        _sh(["zfs", "set", f"mountpoint={path}", ds], run)
        _sh(["zfs", "set", "canmount=on", ds], run)
        if run(["findmnt", "-n", "-o", "SOURCE", path], capture_output=True, text=True).stdout.strip() != ds:
            _sh(["zfs", "mount", ds], run)
        if findmnt_source(path, run) != ds:
            raise RuntimeError(f"{ds} did not mount at {path}")
    except Exception:
        run(["zfs", "unmount", ds], capture_output=True, text=True)
        shutil.rmtree(path, ignore_errors=True)
        os.rename(stash, path)
        raise
    run(["zfs", "inherit", MIGRATING_PROP, ds], capture_output=True, text=True)
    try:
        os.rmdir(tmp)
    except OSError:
        pass
    if keep_original:
        say(f"  kept the original at {stash} (remove it yourself once you have checked the VMs)")
    else:
        shutil.rmtree(stash, ignore_errors=True)
        say(f"  removed the old copy; the original files stay in snapshot {snapshot or '(none)'} until you destroy it")
    return ds


def migrate(c, dry_run=False, keep_original=False, paths=None, boot="/boot", snapshot_cmd=None, run=subprocess.run, say=print):
    """returns the process exit code (0 done/nothing to do, 3 blocked)"""
    rows = plan(c, paths, run)
    todo = [r for r in rows if r["action"] in ("migrate", "resume")]
    for r in rows:
        tag = {"migrate": "MOVE", "resume": "RESUME", "skip": "ok  "}[r["action"]]
        say(f"{tag} {r['path']:<26} -> {r['dataset'] if r['action'] != 'skip' else '':<34} {r['why'] or str(r['bytes'] >> 20) + ' MB'}")
        for b in r["blockers"]:
            say(f"     BLOCKED: {b}")
    blocked = [r for r in rows if r["blockers"]]
    if blocked:
        say("nothing was changed: fix the blockers above and run again" if not dry_run else "dry run: blocked, see above")
        return 3
    if not todo:
        say("nothing to migrate: every data path is already outside the boot environment dataset (or empty/absent)")
        return 0
    if dry_run:
        say("dry run: nothing changed; run without --dry-run to apply")
        return 0
    snap = snapshot_cmd(f"migrate-data-{time.strftime('%Y%m%d-%H%M%S')}") if snapshot_cmd else None
    from . import befallback
    for r in todo:
        say(f"migrating {r['path']}")
        migrate_one(c, r, keep_original, run, say, snap)
        befallback.record_event("migrated", path=r["path"], dataset=r["dataset"], snapshot=snap, detail=f"{r['path']} moved onto its own dataset {r['dataset']}")
    say("done: these paths now survive rollbacks. Old boot environments still contain their own (stale) copy under the mount; "
        "destroy them when you no longer need them to reclaim that space.")
    return 0
