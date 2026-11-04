"""'Test machine: NO disk encryption' (owner decision 2026-11-04 for gringotts / hogshead / madammalkins while they are test machines).

An explicit, warned, NON-default choice in the disk stage and in the answer file (`[disk] test_machine_no_encryption = true`):
  - ZFS: the pool is created without native encryption; the hibernation swap is a PLAIN swap partition (p3), resume still runs
    before the pool import (dracut `resume` module); no passphrase prompt exists, the machine boots unattended.
  - xfs-luks (the small-RAM / no-ZFS fallback): plain XFS (no LUKS, no LVM), plain swap partition from portableswap.
  - package-owned: the target gets `package.use/horcrux-test-machine` (`sys-kernel/horcrux-unlock -encrypted`, the same file
    app-misc/horcrux-test-machine ships) so the next `emerge` replaces the unlock module's dracut config with the unencrypted one.
Nothing else changes: no autologin (the login screen still asks for the password), root locked, separate passwords.
An UNENCRYPTED ZFS desktop/laptop install without this flag is refused: nobody ends up unencrypted by accident.
"""
KEY = "test_machine_no_encryption"

WARNING = ("TEST MACHINE: DISK ENCRYPTION IS OFF. Anyone with the machine can read every file on it. "
           "Use this only for a machine that holds no data you would mind losing control of; "
           "to turn encryption back on, reinstall with encryption enabled (see /usr/share/doc/horcrux-test-machine).")

PACKAGE_USE = ("# owned by app-misc/horcrux-test-machine (written by the installer until that package is merged)\n"
               "sys-kernel/horcrux-unlock -encrypted\n")

DRACUT_OMIT = 'omit_dracutmodules+=" crypt dm horcrux-unlock "\n'


def requested(c):
    return bool(c.get(KEY))


def problems(c):
    """pure: list of refusal reasons (unit tested)"""
    errs = []
    if requested(c) and c.get("zfs_encrypt"):
        errs.append("test machine: no encryption contradicts encryption = true (choose one)")
    if requested(c) and c.get("fs") not in (None, "zfs", "xfs-luks", "xfs"):
        errs.append(f"test machine: no encryption applies to ZFS and the XFS fallback, not {c.get('fs')}")
    if (not requested(c) and c.get("install_type") == "desktop" and c.get("fs") == "zfs" and not c.get("zfs_encrypt")):
        errs.append("an unencrypted ZFS desktop/laptop install needs the explicit choice 'test machine: no encryption' "
                    f"([disk] {KEY} = true): disk encryption is the default for laptops")
    return errs


def normalize(c):
    """after problems(): make the config consistent (idempotent)"""
    if not requested(c):
        return c
    c["zfs_encrypt"], c["zfs_passphrase"], c["luks_passphrase"] = False, "", ""
    if c.get("fs") == "xfs-luks":
        c["fs"], c["fs_why"] = "xfs", "test machine: no encryption (XFS without LUKS)"
    return c


def bootconf(swap_uuid=""):
    """dracut config of the unencrypted install (pure): resume (when there is a hibernation swap) + zfs, never the unlock module"""
    mods = "resume zfs" if swap_uuid else "zfs"
    return ("# written by live-installer (test machine: NO disk encryption): resume from the plain swap BEFORE the pool import\n"
            f'add_dracutmodules+=" {mods} "\n' + DRACUT_OMIT)
