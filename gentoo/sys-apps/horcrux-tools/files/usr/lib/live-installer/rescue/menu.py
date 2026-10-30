"""Rescue menu: plain line-oriented text, identical on the VGA console and on a serial line.

  live-rescue [rescue|forensic]

Normal mode finds the installed systems (unlocking LUKS and importing ZFS pools on request), mounts the one you pick
and offers: shell in chroot, reset the admin password, restore a config backup, reinstall the boot loader,
file system check. Forensic mode mounts nothing by itself and offers read-only tools only.
"""
import os
import subprocess
import sys

from liveinst import system as S
from liveinst.consolemenu import Console
from . import scan as SC
from .ops import Rescue, RescueError, admin_twofactor_state, clear_admin_twofactor


class Menu:
    def __init__(self, profile, forensic=False, inp=input, out=None, rescue=None):
        self.p = profile
        self.forensic = forensic
        self.inp = inp
        self.out = out or sys.stdout
        self.title = profile.get("title", "Rescue")
        self.r = rescue or Rescue(profile, forensic, say=self.say, ask=inp)
        self.cur = None             # the selected (mounted) system record
        self.con = Console(profile, inp=inp, out=self.out)

    # -- io ---------------------------------------------------------------
    def say(self, s=""):
        print(s, file=self.out, flush=True)

    def ask(self, prompt, default=""):
        v = (self.inp(prompt + (f" [{default}]" if default else "") + ": ") or "").strip()
        return v or default

    def yes(self, prompt, default=False):
        v = self.ask(prompt + (" [Y/n]" if default else " [y/N]")).lower()
        return default if not v else v[0] == "y"

    def pause(self):
        try:
            self.inp("\nPress Enter to return to the menu ")
        except EOFError:
            pass

    # -- display ----------------------------------------------------------------
    def banner(self):
        self.say()
        self.say("=" * 64)
        if self.forensic:
            self.say(f" {self.title} FORENSIC RESCUE (no-touch)")
            self.say(" Every disk is READ-ONLY at the block layer. Nothing is mounted")
            self.say(" automatically, swap is off. Mounts and ZFS imports are read-only.")
        else:
            self.say(f" {self.title} RESCUE MODE")
        self.say("=" * 64)

    def list_systems(self):
        if not self.r.systems:
            self.say("No installed system was found yet.")
            return
        self.say("Installed systems found:")
        for i, s in enumerate(self.r.systems, 1):
            tag = f"  [{self.title}]" if s.get("product") else ""
            self.say(f"  {i}) {s['pretty'] or 'Linux system'}  on {s['dev']} ({s['fstype']}{(', ' + S.fmt_size(s['size'])) if s.get('size') else ''}){tag}")

    def show_disks(self):
        rc, out = S.sh(["lsblk", "-o", "NAME,SIZE,TYPE,FSTYPE,LABEL,RO,MOUNTPOINT"])
        self.say(out.rstrip())

    # -- discovery ----------------------------------------------------------------
    def discover(self):
        r = self.r
        self.say("Looking for installed systems ...")
        for d in r.locked_volumes():
            if self.yes(f"{d['path']} is an encrypted (LUKS) volume. Unlock it", True):
                m = r.unlock(d["path"])
                self.say("  unlocked as " + m if m else "  could not unlock (wrong passphrase?)")
        r.activate_lvm()
        for pool in r.zpools():
            if self.yes(f"ZFS pool '{pool['name']}' ({pool['state']}) can be imported. Import it", True):
                self.import_zfs(pool["name"])
        r.scan()
        r.systems.extend(self.zfs_found)
        self.list_systems()

    zfs_found = []

    def import_zfs(self, name):
        r = self.r
        try:
            try:
                r.import_pool(name)
            except RescueError as e:
                if "force" in str(e).lower() or "hostid" in str(e).lower() or "last accessed" in str(e).lower():
                    if not self.yes("The pool was last used by another system. Import anyway (-f)", True):
                        return
                    r.import_pool(name, force=True)
                else:
                    raise
            recs = r.zfs_systems(name)
            if not recs:
                self.say(f"Pool {name} imported; no dataset mounts at '/'.")
            for rec in recs:
                self.say(f"  pool {name}: root dataset {rec['dev']}")
                self.zfs_found = self.zfs_found + [rec]
        except RescueError as e:
            self.say("ZFS import failed: " + str(e))

    # -- choose + mount ----------------------------------------------------------------
    def choose(self):
        if self.cur:
            self.r.unmount_system()
            self.cur = None
        if not self.r.systems:
            self.say("Nothing to choose from. Use 7 to unlock a volume or import a pool, then rescan.")
            return None
        if len(self.r.systems) == 1:
            v = "1"
        else:
            self.list_systems()
            v = self.ask("System to work on (number, empty cancels)")
        if not v.isdigit() or not 1 <= int(v) <= len(self.r.systems):
            self.say("Cancelled.")
            return None
        rec = self.r.systems[int(v) - 1]
        try:
            root = self.r.mount_system(rec)
        except RescueError as e:
            self.say("Mount failed: " + str(e))
            return None
        self.cur = rec
        self.say(f"Mounted {rec['pretty'] or rec['dev']} at {root}" + (" (READ-ONLY)" if self.forensic else ""))
        return rec

    def need_system(self):
        if self.cur:
            return self.cur
        if len(self.r.systems) >= 1:
            return self.choose()
        self.say("No system is mounted." + (" Mount one first (option 2)." if self.forensic else " Use 6 to choose one."))
        return None

    # -- actions ---------------------------------------------------------------
    def do_shell(self):
        rec = self.need_system()
        if not rec:
            return
        self.say("Shell inside the installed system" + (" (its root is READ-ONLY)" if self.forensic else "") + ". Type 'exit' to return.")
        self.r.chroot_shell(rec["root"])

    def do_password(self):
        rec = self.need_system()
        if not rec:
            return
        self.say("Reset the web administrator ('admin') password of the installed system. Empty input cancels.")
        pw = self.con.new_password()
        if not pw:
            self.say("Cancelled.")
            return
        also_root = self.yes("Use the same password for the root console login", True)
        tf = []
        try:
            tf = [v for _, v in admin_twofactor_state(self.p, rec["root"]) if v not in (None, 0, "", "0")]
        except Exception:  # noqa
            tf = []
        try:
            self.say(self.r.reset_password(rec, pw, also_root))
            if tf and self.yes("Two-factor authentication is enabled for admin. Disable it too (lost token)", False):
                clear_admin_twofactor(self.p, rec["root"])
                self.say("Two-factor authentication disabled for admin.")
            self.say("PASSWORD-RESET-DONE")
        except Exception as e:  # noqa
            self.say("Password reset failed: " + str(e))

    def do_restore(self):
        rec = self.need_system()
        if not rec:
            return
        found = self.r.find_backups(rec["root"])
        if not found:
            self.say(f"No backups (*.tar.gz) in {rec['root']}/var/backups/{self.p['name']} or on an attached USB stick.")
            return
        for i, (f, src) in enumerate(found, 1):
            self.say(f"  {i}) {os.path.basename(f)}   [{src}]")
        v = self.ask("Backup to restore (number, empty cancels)")
        if not v.isdigit() or not 1 <= int(v) <= len(found):
            self.say("Cancelled.")
            return
        if self.ask("This REPLACES the configuration of the installed system (a copy of it is saved first). Type RESTORE").strip() != "RESTORE":
            self.say("Cancelled.")
            return
        try:
            snap = self.r.restore_backup(rec, found[int(v) - 1][0])
            self.say("Saved the previous configuration as " + snap)
            self.say("Configuration restored; it takes effect when the system boots. RESTORE-DONE")
        except Exception as e:  # noqa
            self.say(str(e))

    def do_bootloader(self):
        rec = self.need_system()
        if not rec:
            return
        try:
            target, efi, disks = self.r.bootloader_plan(rec)
        except RescueError as e:
            self.say(str(e))
            return
        self.say(f"Firmware: {'UEFI' if efi else 'BIOS'}  grub target: {target}" + ("" if efi else f"  disk(s): {', '.join(disks) or 'unknown'}"))
        d = disks
        if not efi:
            v = self.ask("Install GRUB to disk(s) (space separated)", " ".join(disks))
            d = v.split()
        if not self.yes("Reinstall the boot loader and regenerate grub.cfg", True):
            return
        try:
            for line in self.r.reinstall_bootloader(rec, d):
                self.say("  " + line)
            self.say("Boot loader reinstalled. BOOTLOADER-DONE")
        except RescueError as e:
            self.say("Failed: " + str(e))

    def do_fsck(self):
        rec = self.need_system()
        if not rec:
            return
        self.say("The file system is unmounted for the check" + (" (read-only check, nothing is repaired)." if self.forensic else "."))
        rc, out = self.r.fsck(rec)
        self.say(out.strip()[-1500:])
        self.say(f"Check finished (exit {rc}). FSCK-DONE")
        self.cur = None
        if self.yes("Mount the system again", True):
            try:
                self.r.mount_system(rec)
                self.cur = rec
            except RescueError as e:
                self.say(str(e))

    def do_unlock_import(self):
        locked = self.r.locked_volumes()
        for d in locked:
            if self.yes(f"Unlock {d['path']}", False):
                m = self.r.unlock(d["path"])
                self.say("  unlocked as " + m if m else "  could not unlock")
        self.r.activate_lvm()
        for pool in self.r.zpools():
            if self.yes(f"Import ZFS pool '{pool['name']}'" + (" (read-only)" if self.forensic else ""), False):
                self.import_zfs(pool["name"])
        if not self.forensic:
            self.r.scan()
            self.r.systems.extend(x for x in self.zfs_found if x not in self.r.systems)
            self.list_systems()

    def live_shell(self):
        self.say("Live system shell. Type 'exit' to return.")
        subprocess.call(["/bin/bash", "-l"])

    # forensic-only -------------------------------------------------------------------
    def f_mount(self):
        devs = [d for d in self.r.devices() if SC.classify(d) in ("fs", "efi") or (d.get("fstype") in ("ntfs", "exfat", "btrfs", "ext2", "ext3"))]
        live = self.r.live_set(self.r.devices())
        devs = [d for d in devs if d["path"] not in live]
        for d in list(self.r.locked_volumes()):
            self.say(f"  (encrypted volume {d['path']}: use 6 to unlock it read-only)")
        if not devs:
            self.say("No mountable file system found.")
            return
        for i, d in enumerate(devs, 1):
            self.say(f"  {i}) {d['path']}  {d['fstype']}  {S.fmt_size(d['size'])}  {d.get('label') or ''}")
        v = self.ask("Mount which one read-only (number, empty cancels)")
        if not v.isdigit() or not 1 <= int(v) <= len(devs):
            return
        d = devs[int(v) - 1]
        rec = {"kind": "fs", "dev": d["path"], "fstype": d["fstype"], "label": d.get("label") or "", "uuid": d.get("uuid") or "",
               "pretty": "", "product": False, "size": d.get("size", 0), "root": ""}
        try:
            root = self.r.mount_system(rec)
            rec["pretty"] = SC.os_pretty(S.read(root + "/etc/os-release"))
            self.cur = rec
            self.say(f"Mounted READ-ONLY at {root}")
        except RescueError as e:
            self.say("Mount failed: " + str(e))

    def f_hash(self):
        dev = self.ask("Device to hash (e.g. /dev/sda or /dev/sda1)")
        if not dev.startswith("/dev/") or not os.path.exists(dev):
            self.say("No such device.")
            return
        self.say("Hashing " + dev + " (this reads the whole device) ...")
        self.say(f"SHA256 {dev} {self.r.hash_device(dev)}")

    def f_allow_write(self):
        dev = self.ask("Device to make WRITABLE for saving evidence (e.g. /dev/sdc1; empty cancels)")
        if not dev:
            return
        if self.ask(f"Type WRITABLE to allow writes to {dev} (this disables the protection for that device only)") != "WRITABLE":
            self.say("Cancelled.")
            return
        rc, o = self.r.allow_write(dev)
        self.say("done" if rc == 0 else o)

    def f_umount(self):
        self.r.unmount_system()
        self.r.close_luks()
        self.cur = None
        self.say("Everything unmounted.")

    def reboot(self):
        if self.yes("Reboot now", False):
            self.r.unmount_system()
            self.r.close_luks()
            subprocess.call(["systemctl", "--no-block", "reboot"] if os.path.exists("/run/systemd/system") else ["/sbin/reboot"])

    # -- loop ---------------------------------------------------------------------------
    def menu_text(self):
        cur = (self.cur["pretty"] or self.cur["dev"]) + f" on {self.cur['dev']}" if self.cur else "none selected"
        if self.forensic:
            return f"""
Forensic rescue menu   [system: {cur}]
  1) Show disks and file systems
  2) Mount a file system read-only
  3) Shell in the mounted system (read-only chroot)
  4) Check the file system (read-only, no repair)
  5) SHA-256 of a disk or partition
  6) Unlock a LUKS volume / import a ZFS pool (read-only)
  7) Allow writing to ONE device (to save evidence)
  8) Unmount everything
  9) Reboot
  0) Exit to the live shell"""
        return f"""
Rescue menu   [system: {cur}]
  1) Shell in the installed system (chroot)
  2) Reset the admin password
  3) Restore a configuration backup
  4) Reinstall the boot loader
  5) File system check
  6) Choose another system / rescan
  7) Unlock an encrypted volume / import a ZFS pool
  8) Live shell (no chroot)
  9) Reboot
  0) Exit to the live shell"""

    def loop(self):
        self.banner()
        if self.forensic:
            self.say("Nothing has been mounted. Disks:")
            self.show_disks()
        else:
            self.discover()
            if len(self.r.systems) == 1:
                self.choose()
        while True:
            self.say(self.menu_text())
            try:
                v = self.ask("Enter an option").strip()
            except (EOFError, KeyboardInterrupt):
                break
            if v == "0":
                break
            fn = ({"1": self.show_disks, "2": self.f_mount, "3": self.do_shell, "4": self.do_fsck, "5": self.f_hash, "6": self.do_unlock_import,
                   "7": self.f_allow_write, "8": self.f_umount, "9": self.reboot} if self.forensic else
                  {"1": self.do_shell, "2": self.do_password, "3": self.do_restore, "4": self.do_bootloader, "5": self.do_fsck,
                   "6": self.rescan_choose, "7": self.do_unlock_import, "8": self.live_shell, "9": self.reboot}).get(v)
            if not fn:
                self.say("Unknown option.")
                continue
            try:
                fn()
            except (EOFError, KeyboardInterrupt):
                break
            except Exception as e:  # noqa
                self.say(f"Error: {e}")
        self.r.unmount_system()
        self.r.close_luks()
        self.say("Left the rescue menu; everything is unmounted.")
        return 0

    def rescan_choose(self):
        self.r.unmount_system()
        self.cur = None
        self.r.scan()
        self.r.systems.extend(self.zfs_found)
        self.choose()


def main(argv=None):
    argv = argv or []
    forensic = (argv[:1] == ["forensic"]) or os.path.exists("/run/live-installer/forensic")
    if os.geteuid() != 0:
        print("The rescue menu must run as root.")
        return 1
    return Menu(S.load_profile(), forensic).loop()
