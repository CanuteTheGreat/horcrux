#!/usr/bin/env python3
"""Real-file-system test of horcrux-be's /boot handling: 30 snapshots (a new 75 MB kernel every 3rd) on a 512 MB ext4 /boot
must never exceed 80% used and never raise. Needs root + loop devices (run on a CI host / in a --privileged container);
`zfs` is faked (tests/test_be_boot_space.FakeZfs), everything on /boot is real."""
import os
import subprocess
import sys
import tempfile
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "overlay", "usr", "lib", "live-installer"))
sys.path.insert(0, os.path.join(HERE, "tests"))
from liveinst import be as BE  # noqa: E402
from test_be_boot_space import FakeZfs  # noqa: E402

MB = 1 << 20


def main():
    work = tempfile.mkdtemp(prefix="beboot-")
    img, mnt = os.path.join(work, "boot.img"), os.path.join(work, "boot")
    os.makedirs(mnt)
    subprocess.run(["truncate", "-s", "512M", img], check=True)
    subprocess.run(["mkfs.ext4", "-q", "-F", img], check=True)
    subprocess.run(["mount", "-o", "loop", img, mnt], check=True)
    try:
        z = FakeZfs()
        c = BE.conf("/nonexistent")
        c["KEEP"] = "50"
        os.makedirs(os.path.join(mnt, "be", "horcrux"))

        def used():
            st = os.statvfs(mnt)
            return 100.0 * (st.f_blocks - st.f_bfree) / st.f_blocks

        def kernel(kv):
            for old in os.listdir(mnt):
                if old.startswith(("vmlinuz-", "initramfs-")):
                    os.unlink(os.path.join(mnt, old))
            with open(os.path.join(mnt, f"vmlinuz-{kv}"), "wb") as f:  # 62 MB kernel + 13 MB initramfs = the real 75 MB set
                f.write(os.urandom(62 * MB))
            with open(os.path.join(mnt, f"initramfs-{kv}.img"), "wb") as f:
                f.write(os.urandom(13 * MB))

        worst, snaps = 0.0, 0
        with mock.patch.object(BE.subprocess, "run", z.run), mock.patch.object(BE, "running_be", lambda: "horcrux"), \
                mock.patch.object(BE, "active_be", lambda p: "horcrux"), mock.patch.object(BE, "grubcfg"):
            kernel("7.2.0")
            BE.cmd_sync(c, mnt)
            for i in range(30):
                if i and i % 3 == 0:
                    kernel(f"7.2.{i}")
                    BE.cmd_sync(c, mnt)
                    worst = max(worst, used())
                BE.cmd_snapshot(c, f"pre-upgrade-20261105-{100000 + i}", mnt)
                snaps += 1
                worst = max(worst, used())
                print(f"snapshot {i + 1:2d}: /boot {used():5.1f}% used, {len(z.snaps)} snapshots kept", flush=True)
        left = len(os.listdir(os.path.join(mnt, "be", ".snap")))
        print(f"RESULT snapshots_made={snaps} kept={left} worst_used={worst:.1f}%")
        assert worst <= 80.0, f"/boot reached {worst:.1f}%"
        assert left >= 2, "pruned too much"
        assert os.path.getsize(os.path.join(mnt, "be", "horcrux", "vmlinuz")) == 62 * MB
        print("ok - 30 snapshots on a 512 MB /boot never exceeded 80%")
        return 0
    finally:
        subprocess.run(["umount", mnt], check=False)


if __name__ == "__main__":
    sys.exit(main())
