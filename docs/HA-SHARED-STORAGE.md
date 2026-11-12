# HA guests on NFS shared storage: set a short NFSv4 lease

When a node dies, its QEMU processes die with it, but the NFS server still holds the
dead client's file locks (including the image lock QEMU takes on every VM disk) until the
client's **NFSv4 lease** expires. The Linux server default is **90 seconds**, so a guest
could not be restarted on a survivor for 90 s after a node crash, whatever the HA timers
are set to. In the 3-VM cluster HA suite the same crash took 95 s to recover with the
default lease and 38 s with a 15 s lease.

* **Horcrux NAS NFS server**: writes `lease-time` and `grace-time` (default 20 s,
  `NfsGlobalConfig.lease_secs`, clamped to 10..3600) to `/etc/nfs.conf` itself.
* **Any other NFS server** that exports HA shared storage: set the same two keys and
  restart `nfs-server` (a restart starts a grace period, so do it in a quiet moment):

  ```ini
  # /etc/nfs.conf.d/horcrux-ha.conf
  [nfsd]
  lease-time = 20
  grace-time = 20
  ```

  Check with `cat /proc/fs/nfsd/nfsv4leasetime`. Do not go below 10 s: clients renew at a
  third of the lease and a busy server or one lost packet must not expire healthy clients.
