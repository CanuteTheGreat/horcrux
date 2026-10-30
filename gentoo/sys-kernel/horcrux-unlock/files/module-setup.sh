#!/usr/bin/env bash
# horcrux-unlock: ONE passphrase prompt for an encrypted-ZFS laptop that also has a LUKS2 hibernation swap partition.
#
#   boot order inside this initramfs (OpenRC image, so no systemd units, only dracut hooks):
#     cmdline          91 parse-hx   : udev rule + wait for /dev/mapper/hxswap  (kernel arg rd.hx.swap=<LUKS UUID>)
#     initqueue/settled hx-unlock    : ask the passphrase ONCE, `cryptsetup open` the swap with it, keep it in /run/hx/key (0600 tmpfs)
#     udev (74resume)  swsuspend     : /dev/mapper/hxswap holds a hibernation image -> `echo maj:min > /sys/power/resume` => RESUME
#                                      (this happens BEFORE any pool import: importing a pool and then resuming an older memory image
#                                       that believes the pool is exported corrupts it)
#     pre-mount 10     74resume      : the stock resume hook (no-op when no image)
#     pre-mount 50     hx-zfskey     : only now: `zpool import`, `zfs load-key -L file:///run/hx/key`, key file shredded
#     mount 98         90zfs         : mounts the root; the key is already loaded so it does not prompt a second time
#   If anything fails (wrong swap passphrase, no key) the stock ZFS prompt is the fallback.
check() { return 255; }
depends() { echo crypt dm resume zfs; }
installkernel() { instmods dm_crypt =crypto; }
install() {
    inst_multiple cryptsetup stty flock shred
    inst_hook cmdline 40 "$moddir/parse-hx.sh"
    inst_script "$moddir/hx-unlock.sh" /sbin/hx-unlock
    inst_script "$moddir/hx-try.sh" /lib/hx-try
    inst_hook pre-mount 50 "$moddir/hx-zfskey.sh"
    inst_hook cleanup 10 "$moddir/hx-cleanup.sh"
}
