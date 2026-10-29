#!/usr/bin/env python3
"""qemu-test.py - boot the hogshead live ISO in QEMU/OVMF the way the laptop will (USB stick, UEFI) and assert.

  qemu-test.py ISO --out DIR [--tskey FILE] [--fw-sha SHA] [--expect-keys N] [--kvm]

Needs: qemu-system-x86_64, OVMF, ssh/ssh-keygen/sshpass, mkfs.vfat + mtools, python3. Runs as a normal user.
 - USB boot: -drive if=none,id=usb,file=ISO ... -device usb-storage,bootindex=1 (what the Option-key 'EFI Boot' stick is)
 - HSKEYS FAT disk (label HSKEYS) carries the TEST ssh public key (+ optional TEST tailscale key) = the documented key-import path
 - an emulated NVMe disk plays the old LUKS/LVM/ext4 system for hogshead-unlock / hogshead-backup
 - guest -> checks over ssh (guest-checks.sh), plus serial-console assertions (autologin, MOTD, passphrase prompt)
"""
import argparse, os, re, shutil, socket, subprocess, sys, threading, time

ap = argparse.ArgumentParser()
ap.add_argument('iso'); ap.add_argument('--out', required=True)
ap.add_argument('--tskey'); ap.add_argument('--fw-sha', default=''); ap.add_argument('--expect-keys', default='')
ap.add_argument('--kvm', action='store_true'); ap.add_argument('--port', type=int, default=2222)
ap.add_argument('--boot-timeout', type=int, default=600)
a = ap.parse_args()
OUT = os.path.abspath(a.out); os.makedirs(OUT, exist_ok=True)
HERE = os.path.dirname(os.path.abspath(__file__))
LOG = open(os.path.join(OUT, 'qemu-test.log'), 'w')
fails = []
def log(s):
    print(s, flush=True); LOG.write(s + '\n'); LOG.flush()
def sh(cmd, **kw):
    return subprocess.run(cmd, shell=isinstance(cmd, str), capture_output=True, text=True, **kw)

# ---- test key + HSKEYS stick + fake old disk
key = os.path.join(OUT, 'testkey')
for f in (key, key + '.pub'):
    if os.path.exists(f): os.remove(f)
sh(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'hogshead-qemu-test', '-f', key])
hsk = os.path.join(OUT, 'hskeys.img')
sh(f'dd if=/dev/zero of={hsk} bs=1M count=8 status=none && mkfs.vfat -n HSKEYS {hsk} >/dev/null && mcopy -i {hsk} {key}.pub ::authorized_keys')
if a.tskey: sh(f'mcopy -i {hsk} {a.tskey} ::ts-authkey')
nvme = os.path.join(OUT, 'old-nvme.img'); sh(['truncate', '-s', '1G', nvme])
vars_ = os.path.join(OUT, 'OVMF_VARS.fd')
code = next(p for p in ('/usr/share/OVMF/OVMF_CODE_4M.fd', '/usr/share/OVMF/OVMF_CODE.fd') if os.path.exists(p))
shutil.copy(next(p for p in ('/usr/share/OVMF/OVMF_VARS_4M.fd', '/usr/share/OVMF/OVMF_VARS.fd') if os.path.exists(p)), vars_)

sock = os.path.join(OUT, 'ser.sock')
if os.path.exists(sock): os.remove(sock)
q = ['qemu-system-x86_64', '-machine', 'q35', '-m', '4096', '-smp', '2', '-display', 'none', '-no-reboot',
     '-cpu', 'qemu64,+ssse3,+sse4.1,+sse4.2,+popcnt' if not a.kvm else 'Haswell-noTSX',   # x86-64-v2 class, not -cpu host
     '-drive', f'if=pflash,format=raw,readonly=on,file={code}', '-drive', f'if=pflash,format=raw,file={vars_}',
     '-device', 'qemu-xhci,id=xhci',
     '-drive', f'if=none,id=usb,file={a.iso},format=raw,readonly=on', '-device', 'usb-storage,drive=usb,bootindex=1,bus=xhci.0',
     '-drive', f'if=none,id=hsk,file={hsk},format=raw', '-device', 'virtio-blk-pci,drive=hsk',
     '-drive', f'if=none,id=old,file={nvme},format=raw', '-device', 'nvme,drive=old,serial=hogshead-old',
     '-netdev', f'user,id=n0,hostfwd=tcp:127.0.0.1:{a.port}-:22', '-device', 'virtio-net-pci,netdev=n0',
     '-chardev', f'socket,id=ser0,path={sock},server=on,wait=off,logfile={OUT}/serial.log', '-serial', 'chardev:ser0']
if a.kvm: q += ['-enable-kvm']
else: q += ['-accel', 'tcg']
log('QEMU: ' + ' '.join(q))
proc = subprocess.Popen(q, stdout=subprocess.DEVNULL, stderr=open(os.path.join(OUT, 'qemu.err'), 'w'))

buf = ''; blk = threading.Lock(); conn = None
def connect():
    global conn
    for _ in range(100):
        try:
            c = socket.socket(socket.AF_UNIX); c.connect(sock); conn = c; return
        except OSError: time.sleep(0.2)
    raise SystemExit('no serial socket')
connect()
def reader():
    global buf
    while True:
        try: d = conn.recv(4096)
        except OSError: return
        if not d: return
        with blk: buf += d.decode('utf-8', 'replace')
threading.Thread(target=reader, daemon=True).start()
def send(s): conn.sendall(s.encode())
def expect(rx, timeout, start=0):
    end = time.time() + timeout
    while time.time() < end:
        with blk: m = re.search(rx, buf[start:])
        if m: return m
        if proc.poll() is not None: raise SystemExit('qemu exited early')
        time.sleep(0.5)
    return None
def mark():
    with blk: return len(buf)

SSH = ['ssh', '-i', key, '-p', str(a.port), '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
       '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', '-o', 'LogLevel=ERROR', 'root@127.0.0.1']
def gssh(cmd, timeout=300, env=''):
    return sh(SSH + [env + cmd], timeout=timeout)
def check(name, ok, detail=''):
    log(('PASS ' if ok else 'FAIL ') + name + (' :: ' + detail if detail else ''))
    if not ok: fails.append(name)

try:
    t0 = time.time()
    # 1. UEFI -> GRUB (mirrored on the serial port) -> hotkey 2 = serial-console entry
    m = expect(r'Hogshead - live', 180)
    check('grub-menu-from-usb-uefi', bool(m), 'GRUB menu reached from the USB-attached ISO via OVMF')
    if not m: raise SystemExit('no GRUB')
    time.sleep(1); send('2')
    # 2. kernel + initramfs + live root + OpenRC; autologin console shows the MOTD
    m = expect(r'HOGSHEAD live USB', a.boot_timeout)
    check('boot-to-autologin-console', bool(m), f'{int(time.time()-t0)}s to the MOTD (serial autologin = same getty/profile as tty1)')
    if not m: raise SystemExit('boot failed')
    time.sleep(3)
    with blk: motd = buf[buf.index('HOGSHEAD live USB'):][:1800]
    for s in ('nmtui', 'hogshead-remote', 'hogshead-unlock', 'LAN IP', 'Tailscale IP'):
        check('motd-mentions-' + s, s in motd)
    open(os.path.join(OUT, 'motd.txt'), 'w').write(motd)
    # 3. sshd up (key-only)
    ok = False
    for _ in range(60):
        r = gssh('true', timeout=30)
        if r.returncode == 0: ok = True; break
        time.sleep(3)
    check('sshd-key-login-root', ok, 'root login with the HSKEYS test key')
    pw = sh(['sshpass', '-p', 'root', 'ssh', '-p', str(a.port), '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
             '-o', 'PreferredAuthentications=password', '-o', 'PubkeyAuthentication=no', '-o', 'ConnectTimeout=10', 'root@127.0.0.1', 'true'], timeout=60)
    check('ssh-password-root-refused', pw.returncode != 0 and 'Permission denied' in pw.stderr, pw.stderr.strip()[-120:])
    pw2 = sh(['sshpass', '-p', 'x', 'ssh', '-p', str(a.port), '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
              '-o', 'PreferredAuthentications=password', '-o', 'PubkeyAuthentication=no', '-o', 'ConnectTimeout=10', 'nobody@127.0.0.1', 'true'], timeout=60)
    check('ssh-password-user-refused', pw2.returncode != 0, pw2.stderr.strip()[-120:])
    # 4. guest-side checks
    g = gssh('sh -s', timeout=600, env='') if False else sh(SSH + [f'FW_SHA={a.fw_sha} EXPECT_KEYS={a.expect_keys} sh -s'], input=open(os.path.join(HERE, 'guest-checks.sh')).read(), timeout=600)
    open(os.path.join(OUT, 'guest-checks.txt'), 'w').write(g.stdout + g.stderr)
    for l in g.stdout.splitlines():
        log('  ' + l)
        if l.startswith('FAIL'): fails.append(l.split()[1])
    # 5. old-disk flow: build LUKS+LVM+ext4 on the emulated NVMe (p3, as on the real laptop), then hogshead-unlock + hogshead-backup at the "keyboard"
    prep = r'''set -e
ls /dev/nvme0n1 >/dev/null
sgdisk -Z /dev/nvme0n1 >/dev/null; sgdisk -n1:0:+64M -n2:0:+128M -n3:0:0 -t3:8309 /dev/nvme0n1 >/dev/null; partprobe /dev/nvme0n1; sleep 1
printf 'qemu-test-passphrase' | cryptsetup luksFormat --pbkdf pbkdf2 --pbkdf-force-iterations 1000 -q /dev/nvme0n1p3 -
printf 'qemu-test-passphrase' | cryptsetup open /dev/nvme0n1p3 tmpold -
pvcreate -q /dev/mapper/tmpold; vgcreate -q ubuntu-vg /dev/mapper/tmpold; lvcreate -q -n ubuntu-lv -l 80%FREE ubuntu-vg
mkfs.ext4 -q /dev/ubuntu-vg/ubuntu-lv; mount /dev/ubuntu-vg/ubuntu-lv /mnt
mkdir -p /mnt/home/canutethegreat/Documents /mnt/etc; echo precious > /mnt/home/canutethegreat/Documents/thesis.txt; echo cfg > /mnt/etc/hostname
dd if=/dev/urandom of=/mnt/home/canutethegreat/blob.bin bs=1M count=3 status=none; sha256sum /mnt/home/canutethegreat/blob.bin > /tmp/blob.sha
umount /mnt; vgchange -an ubuntu-vg >/dev/null; cryptsetup close tmpold; echo PREP-OK'''
    r = sh(SSH + ['sh -s'], input=prep, timeout=300)
    check('old-disk-prepared', 'PREP-OK' in r.stdout, (r.stdout + r.stderr)[-200:])
    # refuse over ssh
    r = gssh('hogshead-unlock /dev/nvme0n1p3', timeout=60)
    check('unlock-refuses-over-ssh', r.returncode == 2 and 'laptop' in r.stdout, r.stdout.strip()[:120])
    # at the keyboard (serial console = the local console here): type the passphrase ourselves
    m0 = mark(); send('\n'); time.sleep(1)
    send('hogshead-unlock /dev/nvme0n1p3\n')
    m = expect(r'Enter passphrase|passphrase for', 60, m0)
    check('unlock-prompts-for-passphrase-locally', bool(m))
    send('qemu-test-passphrase\n')
    m = expect(r'Unlocked\. Logical volumes', 90, m0)
    check('unlock-opens-luks-and-lvm', bool(m))
    time.sleep(2)
    r = gssh('hogshead-backup -n /tmp/bk-dry /home/canutethegreat/Documents', timeout=120)
    check('backup-dry-run', r.returncode == 0 and 'thesis.txt' in r.stdout, r.stdout[-200:])
    r = gssh('mkdir -p /tmp/bk && hogshead-backup /tmp/bk /home/canutethegreat /etc/hostname && cat /tmp/bk/home/canutethegreat/Documents/thesis.txt && cat /tmp/bk/etc/hostname && (cd /tmp/bk/home/canutethegreat && sha256sum -c /tmp/blob.sha 2>&1 | head -1; sed "s# .*/# #" /tmp/blob.sha >/dev/null) ; mount | grep hogshead-old', timeout=300)
    check('backup-copies-files', r.returncode == 0 and 'precious' in r.stdout, r.stdout[-300:])
    check('backup-old-root-mounted-readonly', re.search(r'hogshead-old.*\bro\b|hogshead-old.*[(,]ro[,)]', r.stdout) is None, 'unmounted after the run (no -k)')
    r = gssh('hogshead-backup -k -n /tmp/x /etc/hostname >/dev/null; mount | grep hogshead-old; echo test > /mnt/hogshead-old/rw-probe 2>&1; umount /mnt/hogshead-old', timeout=120)
    check('old-root-is-read-only', 'ro' in r.stdout and ('Read-only' in r.stdout or 'read-only' in r.stdout), r.stdout.strip()[-200:])
    # 6. hogshead-remote (only when a TEST tailscale key was given on the HSKEYS stick)
    if a.tskey:
        r = gssh('hogshead-remote', timeout=240)
        open(os.path.join(OUT, 'hogshead-remote.txt'), 'w').write(re.sub(r'tskey-[A-Za-z0-9-]+', 'tskey-***', r.stdout + r.stderr))
        ip = re.search(r'Tailscale IP : (100\.\d+\.\d+\.\d+)', r.stdout)
        check('hogshead-remote-joins-tailnet', bool(ip), (ip.group(1) if ip else (r.stdout + r.stderr)[-300:]))
        lan = re.search(r'LAN IP\(s\)\s*:\s*\n\s+(\d+\.\d+\.\d+\.\d+)', r.stdout)
        check('hogshead-remote-prints-lan-ip', bool(lan), lan.group(1) if lan else '')
        r = gssh('tailscale --socket=/run/tailscale/tailscaled.sock status --json | head -c 0; tailscale --socket=/run/tailscale/tailscaled.sock status | head -3; ps aux | grep -c "[t]skey-"', timeout=60)
        check('authkey-not-in-process-list', r.stdout.strip().splitlines()[-1].strip() == '0', 'ps shows no tskey-')
        gssh('tailscale --socket=/run/tailscale/tailscaled.sock logout', timeout=60)
    else:
        r = gssh('hogshead-remote', timeout=120)
        log('  hogshead-remote with no key/network:\n' + (r.stdout + r.stderr)[-500:])
finally:
    send('\n') if conn else None
    proc.terminate()
    try: proc.wait(20)
    except Exception: proc.kill()
    for f in (nvme, vars_, hsk): 
        if os.path.exists(f): os.remove(f)
    LOG.flush()
log('RESULT: %s' % ('ALL PASS' if not fails else 'FAILED: ' + ', '.join(fails)))
sys.exit(1 if fails else 0)
