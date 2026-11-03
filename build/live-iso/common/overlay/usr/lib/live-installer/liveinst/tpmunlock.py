"""Unattended disk encryption for appliances (Patronus): ZFS native encryption whose key is sealed to the machine's TPM 2.0.

Owner rule (2026-11-03): a firewall must NEVER wait for a human at the console. So encryption is used only when it can be
unlocked without one: a working TPM 2.0 on this hardware -> encrypted pool, key sealed to the TPM (PCR 7 policy), unsealed by
the initramfs (dracut module `tpm-unlock`). No working TPM -> NO encryption and no prompt, ever. The installer decides this
by itself (`decide`) and shows which one it chose and why.

The pool key is 32 random bytes written as 64 hex digits (zfs keyformat=hex). It exists on tmpfs (0600) only during the install.
The same hex string is the RECOVERY KEY shown once at the end of the install: the stock ZFS prompt accepts it if the TPM ever
refuses to unseal (replaced board, firmware change that moves PCR 7).
"""
import os
import secrets
import shutil
import subprocess
import tempfile

PCR = "sha256:7"
DIR = "/etc/tpm-unlock"
MODULE = "tpm-unlock"
TCTI = "device:/dev/tpmrm0"
TOOLS = ("tpm2_createprimary", "tpm2_create", "tpm2_load", "tpm2_unseal", "tpm2_createpolicy", "tpm2_pcrread", "tpm2_startauthsession",
         "tpm2_policypcr", "tpm2_flushcontext")


def new_key():
    return secrets.token_hex(32)        # 64 hex digits = 256 bit


def _run(argv, **kw):
    env = dict(os.environ, TPM2TOOLS_TCTI=TCTI)
    return subprocess.run(argv, capture_output=True, text=True, timeout=kw.pop("timeout", 60), env=env, **kw)


def seal_cmds(workdir, keyfile, pcr=PCR):
    """argv list sealing `keyfile` under a PCR policy (pure: unit tested). Primary key = deterministic ECC primary in the owner hierarchy."""
    w = workdir
    return [
        ["tpm2_createprimary", "-C", "o", "-g", "sha256", "-G", "ecc", "-c", f"{w}/prim.ctx"],
        ["tpm2_pcrread", pcr, "-o", f"{w}/pcr.bin"],
        ["tpm2_createpolicy", "--policy-pcr", "-l", pcr, "-f", f"{w}/pcr.bin", "-L", f"{w}/policy.dat"],
        ["tpm2_create", "-C", f"{w}/prim.ctx", "-i", keyfile, "-L", f"{w}/policy.dat", "-a", "fixedtpm|fixedparent|noda",
         "-u", f"{w}/key.pub", "-r", f"{w}/key.priv"],
    ]


def unseal_cmds(workdir, pubpriv_dir, pcr=PCR):
    w = workdir
    return [
        ["tpm2_createprimary", "-C", "o", "-g", "sha256", "-G", "ecc", "-c", f"{w}/prim2.ctx"],
        ["tpm2_load", "-C", f"{w}/prim2.ctx", "-u", f"{pubpriv_dir}/key.pub", "-r", f"{pubpriv_dir}/key.priv", "-c", f"{w}/key.ctx"],
        ["tpm2_unseal", "-c", f"{w}/key.ctx", "-p", f"pcr:{pcr}", "-o", f"{w}/out.key"],
    ]


def _have_device():
    if os.environ.get("LIVEINST_TPM") in ("0", "1"):
        return os.environ["LIVEINST_TPM"] == "1"
    return os.path.exists("/dev/tpmrm0")


def selftest():
    """(ok, why): a TPM 2.0 that really works = seal a throw-away value to PCR 7 and unseal it again, exactly what the installer and
    the initramfs will do. A TPM that exists but fails this (broken firmware, locked out, disabled) counts as 'no TPM'."""
    if os.environ.get("LIVEINST_TPM") == "0":
        return False, "no TPM 2.0 (forced off)"
    if not os.path.exists("/dev/tpmrm0"):
        return False, "no TPM 2.0 device (/dev/tpmrm0) on this machine"
    miss = [t for t in ("tpm2_createprimary", "tpm2_create", "tpm2_load", "tpm2_unseal", "tpm2_createpolicy", "tpm2_pcrread") if not shutil.which(t)]
    if miss:
        return False, "the image has no tpm2-tools (" + ", ".join(miss) + ")"
    w = tempfile.mkdtemp(prefix="tpmtest")
    try:
        probe = os.path.join(w, "probe.key")
        with open(probe, "w") as f:
            f.write(new_key())
        for argv in seal_cmds(w, probe):
            r = _run(argv)
            if r.returncode:
                return False, f"the TPM cannot seal a key ({argv[0]}: {(r.stderr or r.stdout).strip().splitlines()[-1:] or ['failed']})"
        for argv in unseal_cmds(w, w):
            r = _run(argv)
            if r.returncode:
                return False, f"the TPM cannot unseal a key ({argv[0]}: {(r.stderr or r.stdout).strip().splitlines()[-1:] or ['failed']})"
        if open(os.path.join(w, "out.key")).read() != open(probe).read():
            return False, "the TPM returned a different key than was sealed"
        return True, "a working TPM 2.0 (seal + unseal self-test passed)"
    except (OSError, subprocess.SubprocessError) as e:
        return False, f"the TPM self-test failed: {e}"
    finally:
        shutil.rmtree(w, ignore_errors=True)


def decide(profile, tpm_ok=None):
    """(encrypt: bool, why: str) for a profile with encryption = "tpm-or-none". Never returns an option that needs a typed passphrase."""
    if (profile or {}).get("encryption") != "tpm-or-none":
        return False, "this product profile does not choose encryption by itself"
    ok, why = selftest() if tpm_ok is None else tpm_ok
    if ok:
        return True, "encrypted (ZFS native, AES-256-GCM): the key is sealed to the TPM 2.0, the system boots without a passphrase"
    return False, f"NOT encrypted: {why}. An appliance must boot without anyone typing a passphrase, so without a working TPM there is no encryption"


def dracut_conf():
    return ('# written by the installer: unattended unlock of the encrypted ZFS root (TPM 2.0, dracut module %s)\n'
            'add_dracutmodules+=" %s zfs "\n'
            'add_drivers+=" tpm tpm_tis tpm_tis_core tpm_crb "\n'
            'install_optional_items+=" /usr/lib64/libtss2-tcti-device.so.0 /usr/lib/libtss2-tcti-device.so.0 "\n' % (MODULE, MODULE))


def seal_to_target(inst, key_hex):
    """seal key_hex and write the blobs into the target (/etc/tpm-unlock); returns the recovery key (= key_hex)"""
    w = tempfile.mkdtemp(prefix="tpmseal")
    try:
        kf = os.path.join(w, "in.key")
        fd = os.open(kf, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(key_hex)
        for argv in seal_cmds(w, kf):
            r = _run(argv)
            if r.returncode:
                from .system import InstallError
                raise InstallError(f"TPM sealing failed at {argv[0]}: {(r.stderr or r.stdout).strip()[-300:]}")
        t = f"{inst.target}{DIR}"
        os.makedirs(t, mode=0o700, exist_ok=True)
        for n in ("key.pub", "key.priv"):
            shutil.copyfile(f"{w}/{n}", f"{t}/{n}")
            os.chmod(f"{t}/{n}", 0o600)
        with open(f"{t}/pcr", "w") as f:
            f.write(PCR)
        os.chmod(t, 0o700)
        # prove the blob unseals again before the disk depends on it
        for argv in unseal_cmds(w, t):
            r = _run(argv)
            if r.returncode:
                from .system import InstallError
                raise InstallError(f"TPM unseal check failed at {argv[0]}: {(r.stderr or r.stdout).strip()[-300:]}")
        if open(f"{w}/out.key").read() != key_hex:
            from .system import InstallError
            raise InstallError("TPM unseal returned a different key")
        inst.write("/etc/dracut.conf.d/92-tpm-unlock.conf", dracut_conf())
    finally:
        shutil.rmtree(w, ignore_errors=True)
    return key_hex


def initramfs_problems(lsinitrd_text):
    miss = []
    for need in ("tpm2_unseal", "tpm2_load", "tpm2_createprimary", "etc/tpm-unlock/key.priv", "etc/tpm-unlock/key.pub", "libtss2-tcti-device", "tpm-zfskey"):
        if need not in lsinitrd_text:
            miss.append(need)
    return miss
