"""Rescue operations: find installed systems (ext4/xfs/ZFS, LUKS unlock), mount/import them, and act on them.

Forensic mode (kernel flag <proj>.rescue=forensic, set up by the live-forensic service): nothing is mounted until the
user asks, everything is mounted/imported read-only without journal replay, swap is never enabled, and the
destructive actions (password reset, restore, bootloader, repair) are not offered. The block layer itself is read-only
as well (BLKROSET on every disk), so even a mistake cannot write.
"""
import glob
import json
import os
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile
import time

from liveinst import system as S
from liveinst.consolemenu import Console, argon2_encode
from . import scan as SC

BASE = "/mnt/rescue"
LSBLK_COLS = "NAME,PATH,FSTYPE,LABEL,UUID,SIZE,TYPE,MOUNTPOINT,PKNAME,RM,PARTLABEL,RO"


class RescueError(Exception):
    pass


def sh_long(cmd, timeout=900):
    return S.sh(cmd, timeout=timeout)


def tty_run(cmd):
    """interactive command on the console (passphrase prompts, shells, fsck progress)"""
    try:
        return subprocess.call(cmd, shell=isinstance(cmd, str))
    except OSError as e:
        print("cannot run", cmd, e)
        return 127


# --------------------------------------------------------------------------
# the admin password: the SAME argon2id path as the console menu (liveinst.consolemenu.argon2_encode)
# --------------------------------------------------------------------------
def set_admin_password(profile, root, pw, hasher=argon2_encode, chown=None):
    """Set the web admin password inside the mounted system `root`. Returns 'db' (account updated) or 'file'
    (no account yet: the first-boot password file is written, the product reads it on its first start)."""
    db = root.rstrip("/") + profile.get("db_path", "")
    table = profile.get("db_table", "users")
    if profile.get("db_path") and os.path.exists(db):
        con = sqlite3.connect(db, timeout=20)
        try:
            cur = con.execute(f"UPDATE {table} SET password_hash=? WHERE username='admin'", (hasher(pw),))
            con.commit()
            if cur.rowcount:
                try:
                    con.execute("DELETE FROM sessions")      # drop existing web sessions (Horcrux)
                    con.commit()
                except sqlite3.Error:
                    pass
                return "db"
        finally:
            con.close()
    pf = profile.get("admin_password_file")
    if not pf:
        raise RescueError("this product has no first-boot password file and no admin account was found")
    path = root.rstrip("/") + pf
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(pw + "\n")
    if chown:
        chown(path)
    return "file"


def admin_twofactor_state(profile, root):
    """[(column, value)] of 2FA-looking columns on the admin row (empty = no 2FA state / no DB)"""
    db = root.rstrip("/") + profile.get("db_path", "")
    if not profile.get("db_path") or not os.path.exists(db):
        return []
    table = profile.get("db_table", "users")
    con = sqlite3.connect(db, timeout=20)
    try:
        cols = [r[1] for r in con.execute(f"PRAGMA table_info({table})")]
        tf = SC.twofactor_columns(cols)
        if not tf:
            return []
        row = con.execute(f"SELECT {','.join(tf)} FROM {table} WHERE username='admin'", ()).fetchone()
        return [(c, v) for c, v in zip(tf, row or ())]
    except sqlite3.Error:
        return []
    finally:
        con.close()


def clear_admin_twofactor(profile, root):
    db = root.rstrip("/") + profile.get("db_path", "")
    table = profile.get("db_table", "users")
    con = sqlite3.connect(db, timeout=20)
    try:
        cols = [r[1] for r in con.execute(f"PRAGMA table_info({table})")]
        sets = []
        for c in SC.twofactor_columns(cols):
            sets.append(f"{c}=NULL" if "secret" in c.lower() else f"{c}=0")
        if sets:
            con.execute(f"UPDATE {table} SET {','.join(sets)} WHERE username='admin'")
            con.commit()
        return bool(sets)
    finally:
        con.close()


class Rescue:
    def __init__(self, profile, forensic=False, base=BASE, run=sh_long, tty=tty_run, say=print, ask=input, cmdline=None):
        self.p = profile
        self.forensic = forensic
        self.base = base
        self.run = run
        self.tty = tty
        self.say = say
        self.ask = ask
        self.name = profile.get("name", "product")
        self.title = profile.get("title", self.name)
        self.mounts = []          # mount points we made, in order
        self.imported = []        # ZFS pools we imported
        self.opened = []          # LUKS mappings we opened
        self.systems = []
        self.cmdline = cmdline if cmdline is not None else S.read("/proc/cmdline")
        self.volid = S.read("/etc/live-iso-volid")

    # ------------------------------------------------------------------ discovery
    def devices(self):
        rc, out = self.run(["lsblk", "-J", "-p", "-b", "-o", LSBLK_COLS])
        return SC.parse_lsblk(out) if rc == 0 else []

    def live_set(self, devs):
        return SC.live_medium_devices(devs, self.volid)

    def locked_volumes(self):
        devs = self.devices()
        live = self.live_set(devs)
        kids = {d.get("parent") for d in devs}
        return [d for d in devs if d["path"] not in live and SC.classify(d) == "locked" and d["path"] not in kids]

    def unlock(self, dev, name=None):
        """cryptsetup open (the passphrase is asked by cryptsetup on the console); read-only in forensic mode"""
        name = name or ("rescue-" + os.path.basename(dev))
        cmd = ["cryptsetup", "open"] + (["--readonly"] if self.forensic else []) + [dev, name]
        rc = self.tty(cmd)
        if rc != 0:
            return None
        self.opened.append(name)
        self.run("udevadm settle --timeout=10; true")
        return "/dev/mapper/" + name

    def activate_lvm(self):
        cmd = "vgscan --mknodes >/dev/null 2>&1; vgchange -ay " + ("--readonly " if self.forensic else "") + ">/dev/null 2>&1; udevadm settle --timeout=10; true"
        self.run(cmd)

    def zpools(self):
        if shutil.which("zpool") is None:
            return []
        if not os.path.isdir("/sys/module/zfs"):
            self.run("modprobe zfs 2>/dev/null; true")
        rc, out = self.run(["zpool", "import", "-d", "/dev"])
        return SC.parse_zpool_import(out) if out else []

    def scan(self):
        """list the installed systems found on non-live, non-locked block devices (probes each file system read-only)"""
        self.systems = []
        devs = self.devices()
        live = self.live_set(devs)
        for d in devs:
            if d["path"] in live:
                continue
            if SC.classify(d) == "fs":
                rec = self.probe_fs(d)
                if rec:
                    self.systems.append(rec)
        return self.systems

    def probe_fs(self, d):
        probe = tempfile.mkdtemp(prefix="probe-", dir=self._mk(self.base))
        try:
            rc, o = self.run(["mount", "-o", SC.probe_mount_opts(d["fstype"]), d["path"], probe])
            if rc != 0:
                return None
            try:
                is_sys = (os.path.exists(probe + "/etc/os-release") or os.path.exists(probe + "/etc/fstab")) and \
                         (os.path.isdir(probe + "/usr") or os.path.isdir(probe + "/bin"))
                if not is_sys:
                    return None
                pretty = SC.os_pretty(S.read(probe + "/etc/os-release")) or "Linux system"
                product = os.path.exists(probe + "/etc/live-installer/installed") or os.path.isdir(f"{probe}/etc/{self.name}")
                return {"kind": "fs", "dev": d["path"], "fstype": d["fstype"], "label": d.get("label") or "", "uuid": d.get("uuid") or "",
                        "pretty": pretty, "product": product, "size": d.get("size", 0), "root": ""}
            finally:
                self.run(["umount", probe])
        finally:
            shutil.rmtree(probe, ignore_errors=True)

    def _mk(self, d):
        os.makedirs(d, exist_ok=True)
        return d

    # ------------------------------------------------------------------ ZFS
    def import_pool(self, pool, force=False):
        """import without mounting; forensic: readonly=on (never writes labels/uberblocks)"""
        if not SC.safe_dataset_name(pool):
            raise RescueError("bad pool name")
        altroot = f"{self.base}/zfs-{pool}"
        self._mk(altroot)
        cmd = ["zpool", "import", "-N", "-R", altroot, "-d", "/dev"]
        if self.forensic:
            cmd += ["-o", "readonly=on", "-f"]
        elif force:
            cmd += ["-f"]
        rc, out = self.run(cmd + [pool])
        if rc != 0:
            raise RescueError(out.strip()[-300:] or "zpool import failed")
        self.imported.append(pool)
        return altroot

    def zfs_systems(self, pool):
        altroot = f"{self.base}/zfs-{pool}"
        rc, out = self.run(["zfs", "list", "-H", "-o", "name,mountpoint,canmount,type", "-r", pool])
        roots = SC.zfs_root_datasets(out, altroot)
        recs = []
        for ds in roots:
            recs.append({"kind": "zfs", "dev": ds, "fstype": "zfs", "label": pool, "uuid": "", "pretty": "", "product": False,
                         "size": 0, "root": altroot, "pool": pool})
        return recs

    def zfs_load_keys(self, pool):
        rc, out = self.run(["zfs", "get", "-H", "-r", "-o", "name,value", "keystatus", pool])
        if any(line.endswith("unavailable") for line in out.splitlines()):
            self.say("This pool has encrypted datasets: enter the key when asked.")
            self.tty(["zfs", "load-key", "-r", pool])

    # ------------------------------------------------------------------ mounting
    def mount_system(self, rec):
        """mount the root (+ /boot, /boot/efi and other local fstab file systems); returns the root path"""
        if rec["kind"] == "zfs":
            root = rec["root"]
            self.zfs_load_keys(rec["pool"])
            cmd = ["zfs", "mount"] + (["-o", "ro"] if self.forensic else []) + [rec["dev"]]
            rc, out = self.run(cmd)
            if rc != 0 and "already mounted" not in out:
                raise RescueError(out.strip()[-300:])
            self.mounts.append(root)
            rec["pretty"] = SC.os_pretty(S.read(root + "/etc/os-release")) or rec.get("pretty") or "Linux system"
        else:
            root = f"{self.base}/sys-{os.path.basename(rec['dev'])}"
            self._mk(root)
            opts = SC.ro_mount_opts(rec["fstype"], self.forensic)
            rc, out = self.run(["mount", "-o", opts, rec["dev"], root])
            if rc != 0:
                raise RescueError(out.strip()[-300:] or "mount failed")
            self.mounts.append(root)
        rec["root"] = root
        self.mount_fstab(root)
        self.mount_boot_parts(root, rec)
        return root

    def resolve(self, spec):
        argv = SC.blkid_args(spec)
        if argv is None:
            return spec if spec.startswith("/dev/") else None
        rc, out = self.run(argv)
        out = out.strip().splitlines()
        return out[0] if rc == 0 and out else None

    def mount_fstab(self, root):
        for spec, mp, ft in SC.fstab_mounts(S.read(root + "/etc/fstab")):
            dev = self.resolve(spec)
            if not dev or not os.path.exists(dev):
                self.say(f"  (skipping {mp}: {spec} not found)")
                continue
            target = root.rstrip("/") + mp
            self._mk(target)
            rc, out = self.run(["mount", "-o", SC.ro_mount_opts(ft, self.forensic), "-t", ft, dev, target])
            if rc == 0:
                self.mounts.append(target)
            else:
                self.say(f"  (could not mount {mp}: {out.strip()[-120:]})")

    def mount_boot_parts(self, root, rec):
        """/boot and /boot/efi when the installed fstab does not name them: find the 'boot' and 'ESP' partitions by label"""
        devs = self.devices()
        boot, esp = SC.pick_boot_parts(devs, rec["dev"] if rec["kind"] == "fs" else "", self.live_set(devs))
        for dev, mp, ft in ((boot, "/boot", "ext4"), (esp, "/boot/efi", "vfat")):
            target = root.rstrip("/") + mp
            if not dev or os.path.ismount(target):
                continue
            self._mk(target)
            rc, out = self.run(["mount", "-o", SC.ro_mount_opts(ft, self.forensic), dev, target])
            if rc == 0:
                self.mounts.append(target)
                self.say(f"  mounted {mp} from {dev}")

    def chroot_up(self, root):
        for src, dst, kind in (("/dev", "dev", "rbind"), ("proc", "proc", "proc"), ("/sys", "sys", "rbind"), ("/run", "run", "bind")):
            tgt = f"{root}/{dst}"
            self._mk(tgt)
            if kind == "proc":
                cmd = ["mount", "-t", "proc", "proc", tgt]
            else:
                cmd = ["mount", "--rbind" if kind == "rbind" else "--bind", src, tgt]
            rc, _ = self.run(cmd)
            if rc == 0:
                self.mounts.append(tgt)

    def chroot_down(self, root):
        for dst in ("run", "sys", "dev", "proc"):
            self.run(["umount", "-R", "-l", f"{root}/{dst}"])
        self.mounts = [m for m in self.mounts if not any(m == f"{root}/{d}" for d in ("run", "sys", "dev", "proc"))]

    def chroot_shell(self, root):
        self.chroot_up(root)
        try:
            shell = "/bin/bash" if os.path.exists(root + "/bin/bash") else "/bin/sh"
            env = dict(os.environ, TERM=os.environ.get("TERM", "linux"), HOME="/root", CONSOLE_MENU_INNER="1")   # no console menu inside the chroot
            subprocess.call(["chroot", root, shell, "-l"], env=env)
        finally:
            self.chroot_down(root)

    def chroot_run(self, root, argv, input_=None, timeout=900):
        self.chroot_up(root)
        try:
            r = subprocess.run(["chroot", root] + argv, input=input_, capture_output=True, text=True, timeout=timeout)
            return r.returncode, (r.stdout or "") + (r.stderr or "")
        except Exception as e:  # noqa
            return 1, str(e)
        finally:
            self.chroot_down(root)

    def unmount_system(self, rec=None):
        """unmount everything this session mounted (deepest first); export the ZFS pools it imported"""
        for m in sorted(set(self.mounts), key=len, reverse=True):
            self.run(["umount", "-R", "-l", m] if m.startswith(self.base) else ["umount", m])
        self.mounts = []
        for p in self.imported:
            self.run(["zfs", "umount", "-a"])
            self.run(["zpool", "export", p])
        self.imported = []
        self.run("sync; true")

    def close_luks(self):
        for n in self.opened:
            self.run(["cryptsetup", "close", n])
        self.opened = []

    # ------------------------------------------------------------------ actions
    def reset_password(self, rec, pw, also_root=False):
        if self.forensic:
            raise RescueError("forensic mode never changes the system")
        root = rec["root"]
        how = set_admin_password(self.p, root, pw, chown=lambda path: self._chown_in_root(root, path))
        msg = "web admin password changed (web sessions dropped)" if how == "db" else \
              "no admin account exists yet: the password is applied when the web service first starts"
        if also_root:
            rc, o = self.chroot_run(root, ["chpasswd"], input_=f"root:{pw}\n")
            msg += "; root console password changed" if rc == 0 else "; COULD NOT change root: " + o.strip()[-120:]
        self.fix_owner(root)
        return msg

    def fix_owner(self, root):
        """the product's data directory belongs to its service user (Patronus: patronus): same fix the console menu applies"""
        owner = self.p.get("admin_password_owner", "root")
        db = self.p.get("db_path", "")
        if owner != "root" and db:
            self.chroot_run(root, ["chown", "-R", f"{owner}:{owner}", os.path.dirname(db)])

    def _chown_in_root(self, root, path):
        owner = self.p.get("admin_password_owner", "root")
        rel = path[len(root.rstrip("/")):]
        self.chroot_run(root, ["chown", f"{owner}:{owner}", rel])

    def usb_backup_dirs(self):
        res = []
        devs = self.devices()
        live = self.live_set(devs)
        for i, d in enumerate(devs, 1):
            if d["path"] in live or d.get("mountpoint") or str(d.get("rm")) not in ("1", "True", "true"):
                continue
            if d.get("type") not in ("part", "disk") or SC.classify(d) not in ("fs", "efi") and (d.get("fstype") or "") not in ("exfat", "ntfs"):
                continue
            mnt = f"/mnt/usb-{i}"
            self._mk(mnt)
            if self.run(["mount", "-o", "ro", d["path"], mnt])[0] == 0:
                self.mounts.append(mnt)
                res.append((mnt, d.get("label") or d["path"]))
        return res

    def find_backups(self, root):
        d = f"{root}/var/backups/{self.name}"
        found = [(f, "on the system") for f in sorted(glob.glob(d + "/*.tar.gz"))]
        for mnt, label in self.usb_backup_dirs():
            for f in sorted(glob.glob(f"{mnt}/*.tar.gz") + glob.glob(f"{mnt}/{self.name}*/*.tar.gz")):
                found.append((f, f"USB {label}"))
        return found

    def check_backup(self, path):
        return Console(self.p).check_backup(path)          # manifest, project name, allowed paths, no links/devices

    def snapshot_config(self, root, tag="pre-rescue-restore"):
        d = f"{root}/var/backups/{self.name}"
        os.makedirs(d, mode=0o700, exist_ok=True)
        out = f"{d}/{self.name}-config-{tag}-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}.tar.gz"
        with tarfile.open(out, "w:gz") as tf:
            stage = tempfile.mkdtemp(prefix="cfgbak-")
            try:
                with open(stage + "/MANIFEST.json", "w") as f:
                    json.dump({"project": self.name, "created": tag, "host": "", "format": 1}, f)
                tf.add(stage + "/MANIFEST.json", "MANIFEST.json")
            finally:
                shutil.rmtree(stage, ignore_errors=True)
            for pth in self.p.get("config_paths", []):
                if os.path.exists(root + pth):
                    tf.add(root + pth, pth.lstrip("/"))
        os.chmod(out, 0o600)
        return out

    def restore_backup(self, rec, path):
        if self.forensic:
            raise RescueError("forensic mode never changes the system")
        ok, msg = self.check_backup(path)
        if not ok:
            raise RescueError("refused: " + msg)
        root = rec["root"]
        snap = self.snapshot_config(root)
        db = root + self.p.get("db_path", "")
        for f in (db, db + "-wal", db + "-shm") + tuple(root + x for x in self.p.get("factory_reset_files", [])):
            try:
                os.remove(f)
            except OSError:
                pass
        with tarfile.open(path) as tf:
            members = [m for m in tf.getmembers() if m.name != "MANIFEST.json"]
            tf.extractall(root, members=members, filter="tar")
        self.fix_owner(root)
        return snap

    def bootloader_plan(self, rec):
        """what reinstalling the boot loader would do: (target, efi, disks) - disks only for BIOS"""
        efi = os.path.isdir("/sys/firmware/efi")
        arch = os.uname().machine
        target = SC.grub_target(arch, efi)
        if not target:
            raise RescueError(f"no boot loader target known for {arch} (BIOS boot exists on x86 only)")
        disks = []
        if not efi:
            devs = self.devices()
            by = {d["path"]: d for d in devs}
            cands = [rec["dev"]] if rec["kind"] == "fs" else []
            boot = S.sh(["findmnt", "-n", "-o", "SOURCE", f"{rec['root']}/boot"])[1].strip()
            if boot:
                cands.insert(0, boot)
            if rec["kind"] == "zfs":
                _, st = self.run(["zpool", "status", "-P", rec["pool"]])
                cands += [w for w in st.split() if w.startswith("/dev/")]
            for c in cands:
                d = by.get(c)
                top = d
                while top and top.get("parent") and top["parent"] in by:
                    top = by[top["parent"]]
                if top and top["path"] not in disks and top.get("type") == "disk":
                    disks.append(top["path"])
        return target, efi, disks

    def reinstall_bootloader(self, rec, disks=None):
        if self.forensic:
            raise RescueError("forensic mode never changes the system")
        root = rec["root"]
        target, efi, auto_disks = self.bootloader_plan(rec)
        log = []
        if efi:
            if not os.path.ismount(root + "/boot/efi"):
                raise RescueError("/boot/efi of the installed system is not mounted (no ESP in its fstab?)")
            cmds = [["grub-install", f"--target={target}", "--efi-directory=/boot/efi", f"--bootloader-id={self.title}", "--removable"]]
        else:
            ds = disks or auto_disks
            if not ds:
                raise RescueError("could not tell which disk to put GRUB on")
            cmds = [["grub-install", f"--target={target}", d] for d in ds]
        for c in cmds:
            rc, o = self.chroot_run(root, c)
            log.append(" ".join(c) + f" -> rc={rc}")
            if rc != 0:
                raise RescueError(f"{' '.join(c)} failed: {o.strip()[-300:]}")
        extra = "console=tty0 console=ttyS0,115200n8" if "console=ttyS0" in self.cmdline else ""
        if rec["kind"] == "zfs" and os.path.isdir(root + "/boot/be") and os.path.exists(root + "/usr/bin/horcrux-be"):
            # boot-environment layout (liveinst/be.py): kernels live in /boot/be/<name>/, grub.cfg lists every BE
            rc, o = self.chroot_run(root, ["horcrux-be", "grubcfg"])
            log.append(f"horcrux-be grubcfg -> rc={rc}")
            if rc != 0:
                raise RescueError("horcrux-be grubcfg failed: " + o.strip()[-300:])
        elif rec["kind"] == "zfs":
            kvers = sorted(os.listdir(root + "/lib/modules"))
            boot_dev = self.run(["findmnt", "-n", "-o", "SOURCE", root + "/boot"])[1].strip()
            buuid = S.sh(["blkid", "-s", "UUID", "-o", "value", boot_dev])[1].strip() if boot_dev else ""
            if not kvers or not buuid:
                raise RescueError("cannot write the ZFS grub.cfg (no kernel or no /boot partition)")
            cfg = SC.static_zfs_grubcfg(self.title, kvers[-1], buuid, rec["dev"], extra)
            with open(root + "/boot/grub/grub.cfg", "w") as f:
                f.write(cfg)
            log.append("wrote the static ZFS grub.cfg")
        else:
            rc, o = self.chroot_run(root, ["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])
            log.append(f"grub-mkconfig -> rc={rc}")
            if rc != 0:
                raise RescueError("grub-mkconfig failed: " + o.strip()[-300:])
            uu = rec.get("uuid")
            if uu:      # same fix the installer applies: a device name in root= breaks when disks reorder
                self.run(["sed", "-i", f"s#root={rec['dev']}\\b#root=UUID={uu}#g", root + "/boot/grub/grub.cfg"])
        return log

    def fsck(self, rec):
        """unmount the system, check its file system (forensic: read-only check only), return (rc, output)"""
        self.unmount_system()
        if rec["kind"] == "zfs":
            pool = rec["pool"]
            if self.forensic:
                self.import_pool(pool)
                rc, o = self.run(["zpool", "status", "-v", pool])
                return rc, o
            self.import_pool(pool, force=True)
            rc, o = self.run(["zpool", "scrub", pool])
            for _ in range(600):
                _, st = self.run(["zpool", "status", pool])
                if "scrub in progress" not in st:
                    break
                time.sleep(2)
            return rc, self.run(["zpool", "status", "-v", pool])[1]
        dev, ft = rec["dev"], rec["fstype"]
        if ft in ("ext2", "ext3", "ext4"):
            cmd = ["e2fsck", "-f", "-n" if self.forensic else "-p", dev]
        elif ft == "xfs":
            cmd = ["xfs_repair", "-n", dev] if self.forensic else ["xfs_repair", dev]
        else:
            cmd = ["fsck", "-n" if self.forensic else "-f", dev]
        return self.run(cmd)

    def hash_device(self, dev):
        """SHA-256 of a whole device (chain of custody: run before and after, the values must match)"""
        rc, out = sh_long(["sha256sum", dev], 14400)
        return out.split()[0] if rc == 0 and out.split() else ""

    def allow_write(self, dev):
        """forensic: explicitly unlock ONE device for saving evidence"""
        return self.run(["blockdev", "--setrw", dev])
