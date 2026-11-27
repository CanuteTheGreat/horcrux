# Security: vulnerability scans and the hardening baseline

Security (System menu > Security, or `/security`) has four pages:

- **Overview**: critical and high findings across the newest scan of each target, how many
  targets have been scanned, and the node's hardening score. Every number links to the list
  behind it.
- **Vulnerabilities**: every container image (Podman, Docker), container (LXC, Incus) and virtual
  machine on this node, the newest scan of each, and buttons to scan one or all of them. A scan's
  page lists every finding with the package, the installed version and the version that fixes it.
- **Hardening**: the Horcrux baseline, checked live, with a score, per-rule status, what to do
  for each failing rule, **Apply baseline**, per-rule Apply and Revert, and a downloadable report
  (HTML for people, text, JSON for tools).
- **Settings**: everything below, also settable as the declarative `security:` section.

Design and the comparison with other platforms: [design/security-suite.md](design/security-suite.md).

## Vulnerability scanning

Horcrux runs a free scanner as a separate program:

- **Grype** (Anchore, Apache-2.0), the default. Gentoo: `emerge app-containers/grype`, or build
  Horcrux with `USE=security`, which pulls it in.
- **Trivy** (Aqua Security, Apache-2.0), the alternative: pick it in Settings.

Without the scanner the Vulnerabilities page says so and links here; nothing else is affected.

What is scanned:

| Target | How |
|---|---|
| Podman / Docker image | read through the runtime (`podman:<ref>`, `docker:<ref>`) |
| LXC container | its root file system, `<lxc_path>/<name>/rootfs` (default `/var/lib/lxc`) |
| Incus container | its root file system, `<incus_path>/containers/<name>/rootfs` (default `/var/lib/incus`) |
| VM (stopped) | its disk attached read-only with `qemu-nbd --read-only` and the guest file system mounted read-only, scanned as a directory; the image is never written |
| VM (running) | the guest agent's package list (`horcrux-guest-agent --inventory`), turned into an SBOM the scanner reads; the host never opens the live disk |

Virtual machines are scanned without ever touching a running guest's disk. A **stopped** VM is
scanned from a read-only mount of its first disk (dpkg, rpm, apk, Portage or the Windows file
system); an **encrypted** guest disk (LUKS, BitLocker) is skipped with a clear reason — Horcrux
never unlocks a guest disk, so scan an encrypted guest while it is running instead. A **running**
VM is scanned from the Horcrux guest tools' package inventory over the guest-agent channel, so
nothing on the host reads the guest file system. Turn either off with `vuln_scan.vm_disk_scan`
(stopped) or `vuln_scan.vm_agent_scan` (running).

Scans run one at a time by default, at low CPU priority (niceness 15) and in the idle IO class,
and are stopped after 30 minutes. By default every target is scanned once a day; a restart does
not rescan everything, because the next scan is due a day after the newest one on record.

A finished scan with findings at or above **Notify at or above** (default: critical) sends a
`security` notification through Monitoring > Notifications.

### Nodes without internet

The scanner downloads its vulnerability database on first use and refreshes it before each scan.
For an air-gapped node, copy the database in and turn **Update the database before scanning**
off, or point **Database mirror** at your own copy (Grype: the URL of a `listing.json` /
`latest.json`; Trivy: an OCI repository such as `registry.example/trivy-db`). The database lives
in `<data_dir>/security/scanner-db` unless **db_dir** says otherwise.

## Hardening baseline

The Horcrux baseline (`horcrux-baseline-1`) is the CIS Linux benchmark adapted to a hypervisor:

| Category | Checks |
|---|---|
| kernel | ASLR, kernel pointer and log restriction, Yama ptrace scope, unprivileged eBPF, BPF JIT hardening, set-UID core dumps, protected hard links/symlinks/FIFOs/regular files |
| network | ICMP redirects (accept/send/secure), source routing, martian logging, SYN cookies, broadcast ping, bogus ICMP errors |
| files | modes of /etc/shadow, /etc/gshadow, /etc/passwd, /etc/group, sshd_config, /etc/crontab, the Horcrux config |
| ssh | root login with a password, empty passwords, MaxAuthTries, X11 forwarding |
| accounts | only root has UID 0, no empty passwords |
| mounts | /tmp and /dev/shm options |
| hypervisor | every CPU vulnerability the kernel knows is mitigated |
| services | host firewall loaded, time sync, API access log, auditd, a file integrity checker, no telnet/rsh/tftp servers |

It never turns off IP forwarding, strict reverse-path filtering or user namespaces: guests,
routed networks and rootless/unprivileged containers need them.

**Apply** changes only two kinds of things, and shows each change before it is made:

- kernel settings: written now and to `/etc/sysctl.d/60-horcrux-hardening.conf`, which OpenRC's
  `sysctl` service and `systemd-sysctl` read at every boot;
- file modes: the offending permission bits are removed.

Horcrux records the previous value of each, and **Revert** puts it back (and removes the drop-in
when nothing is left in it). SSH, account, mount and package findings say exactly what to do;
Horcrux never changes those itself, because a wrong SSH change can lock you out.

The baseline is re-checked once a day; a rule that starts failing sends a `security`
notification. The score weights critical rules 8, high 4, medium 2 and the rest 1.

To skip a rule (for example `kernel-ptrace-scope` on a host where you debug processes), add its id
to **Excluded rules**. To require a different value, use an override:

```yaml
security:
  hardening:
    exclude: [kernel-ptrace-scope]
    overrides:
      kernel-aslr: "2"
```

## Pentest VM templates

Security > Pentest templates (also linked from Create Virtual Machine) offers Kali Linux, Parrot Security and Pentoo as ready-made VM templates. Use them only on systems you are authorised to test.

How it works:

1. **Download an image once.** The image goes into the image store (`<data dir>/pentest-images`; an ISO goes into the ISO library). The check has three layers: the SHA-256 pinned in the catalogue (the catalogue itself is signed, see below); the vendor's own published checksum, which must agree with the pin; and, where the vendor signs its checksums (Kali, Parrot), the OpenPGP signature, which must verify against the vendor key fingerprint pinned in the catalogue. Pentoo publishes a checksum but no signature; the page says so. A mismatch removes the file and shows the reason. Progress, Cancel and Delete image are on the page.
2. **Create a VM** from the template: name, network, CPUs, memory and disk (defaults come from the template, then from `pentest.cpus/memory_mb/disk_gb`). The disk image is copied (and grown, never shrunk) into the new VM's own disk.
3. **Network.** The default is an isolated lab: an SDN `simple` zone (`hxlab`) with a VNet (`hxlab0`) that has no subnet, no host address and no uplink, so lab VMs reach each other and nothing else. Horcrux runs no DHCP there: give lab guests static addresses in one subnet. The toggle `LAN` attaches the VM to the host's default-route interface instead (a bridge, or macvtap on a wired port; Wi-Fi is refused with the reason). Set `pentest.network` for the node default and `pentest.lan_interface` to pick the interface.
4. **Unattended install where the distro supports it.** Kali and Parrot are prebuilt images: installed, they boot to the desktop. Pentoo ships as a live ISO: the VM boots it and carries a second disc labelled `HXANSWERS` with `install.sh`, which runs Pentoo's own headless installer (BIOS, one `/boot` and one `/` partition on the virtual disk). The live session has no serial console and no guest agent, so the one command is typed in the live session: `sudo mkdir -p /mnt/hxa && sudo mount -L HXANSWERS /mnt/hxa && sudo sh /mnt/hxa/install.sh` (the session first asks for a password for the live user and whether to start X; answer `n` for a shell). The installer takes about ten minutes; the VM then boots from its disk to a console login (`startx` starts the Xfce desktop). The guest agents are opt-in because Pentoo has no binary package for them and they compile from source: `HX_AGENTS=1 sh /mnt/hxa/install.sh` (needs network, so not on the isolated lab).

Declarative `vms:` can name a template:

```yaml
security:
  pentest:
    prefetch: [kali]        # images download in the background after apply
vms:
  - name: kali-lab
    template: kali
    network: isolated        # or lan; absent = security.pentest.network
    # memory/cpus absent = the template's
    power: running
```

`apply` creates the VM once the image is ready and says "still downloading" otherwise; running it again converges.

### Catalogue updates without a release

The catalogue (URLs, versions, pinned checksums, vendor key fingerprints, hardware defaults) is built into each version and is itself a signed index: `horcrux-common/data/pentest-v1.json` with a detached ed25519 signature (`.sig`, over the exact bytes). Nodes fetch `pentest.catalogue_url` (default: this repository's raw file) every `pentest.refresh_hours`, accept it only if the signature matches a trusted key (`pentest.catalogue_keys`, plus the key compiled in unless `trust_builtin_key` is off), the schema validates and its `serial` is not lower than the one in use (rollback protection). A failed refresh keeps the previous catalogue and shows the error. Point `catalogue_url` at your own mirror and add your own key to publish a private catalogue. Maintainers: `ci/pentest-catalogue.py build|sign|check`.

| Key | Default | Meaning |
|---|---|---|
| `pentest.catalogue_url` | project raw index | signed index URL; empty = built-in only |
| `pentest.catalogue_sig_url` | empty | signature URL; empty = `<catalogue_url>.sig` |
| `pentest.catalogue_keys` | `[]` | extra trusted ed25519 keys (base64, 32 bytes) |
| `pentest.trust_builtin_key` | `true` | trust the key compiled into this version |
| `pentest.refresh_hours` | `24` | hours between refreshes; 0 = on demand |
| `pentest.vendor_signature` | `require` | `require`, `if-available` (skip when gpg is missing) or `off` |
| `pentest.network` | `isolated` | network of new VMs: `isolated` or `lan` |
| `pentest.lab_zone` / `pentest.lab_vnet` | `hxlab` / `hxlab0` | SDN zone and VNet (bridge) of the isolated lab |
| `pentest.lan_interface` | empty | LAN mode interface; empty = the default route's |
| `pentest.store_dir` | empty | image store; empty = `<data dir>/pentest-images` |
| `pentest.keep_images` | `true` | keep the downloaded image after VMs exist |
| `pentest.min_free_gb` | `10` | free space that must remain after a download |
| `pentest.cpus` / `memory_mb` / `disk_gb` | `0` | override the template hardware (0 = the template's) |
| `pentest.prefetch` | `[]` | template ids downloaded in the background |

Needs on the node: `qemu-img`; `xz` (Parrot), a 7-Zip (`app-arch/7zip` or `p7zip`, Kali); `gpg` (`app-crypt/gnupg`) for the vendor signature check; `xorriso`, `genisoimage` or `mkisofs` for Pentoo's answers disc. The page reports what is missing.

| Method and path | Purpose | Privilege |
|---|---|---|
| `GET /api/security/pentest/templates` | catalogue with each image's state and download progress | Sys.Audit |
| `POST /api/security/pentest/templates/:id/download` | fetch + verify the image (202) | Sys.Modify |
| `POST /api/security/pentest/templates/:id/cancel` | stop a running download | Sys.Modify |
| `DELETE /api/security/pentest/templates/:id/image` | remove the stored image | Sys.Modify |
| `POST /api/security/pentest/templates/:id/create` | `{name, network?, cpus?, memory_mb?, disk_gb?, storage?, start?}` creates the VM (201) | Sys.Modify |
| `GET /api/security/pentest/catalogue`, `POST .../catalogue/refresh` | where the catalogue came from; fetch the signed index now | Sys.Audit / Sys.Modify |
| `GET /api/security/pentest/lab` | isolated lab and LAN interface status | Sys.Audit |

## Settings reference

`GET/PUT /api/security/settings`, declarative `security:` (`docs/design/declarative-config.md`).

| Key | Default | Meaning |
|---|---|---|
| `vuln_scan.enabled` | `true` | scanning on for this node |
| `vuln_scan.scanner` | `grype` | `grype` or `trivy` |
| `vuln_scan.scanner_path` | empty | program path; empty = the scanner's name in PATH |
| `vuln_scan.db_url` | empty | database mirror (see above) |
| `vuln_scan.db_dir` | empty | database directory; empty = `<data_dir>/security/scanner-db` |
| `vuln_scan.update_db` | `true` | refresh the database before a scan |
| `vuln_scan.schedule_hours` | `24` | hours between scheduled scans; 0 = on demand only |
| `vuln_scan.schedule_kinds` | all | `podman`, `docker`, `lxc`, `incus`, `vm` |
| `vuln_scan.max_concurrent` | `1` | scans at the same time (1-16) |
| `vuln_scan.nice` | `15` | CPU niceness of the scanner (0-19) |
| `vuln_scan.timeout_secs` | `1800` | a scan that runs longer is stopped |
| `vuln_scan.keep_scans` | `200` | finished scans kept |
| `vuln_scan.only_fixed` | `false` | leave out findings without a fix |
| `vuln_scan.notify_min_severity` | `critical` | `none`, `critical`, `high`, `medium`, `low` |
| `vuln_scan.lxc_path` | `/var/lib/lxc` | LXC container directory |
| `vuln_scan.incus_path` | `/var/lib/incus` | Incus data directory |
| `vuln_scan.vm_disk_scan` | `true` | scan stopped VMs from a read-only disk mount |
| `vuln_scan.vm_agent_scan` | `true` | scan running VMs from the guest agent's inventory |
| `vuln_scan.vm_mount_timeout_secs` | `120` | give up a stopped-VM disk mount after this long (10-3600) |
| `hardening.exclude` | `[]` | rule ids not checked or applied |
| `hardening.overrides` | `{}` | rule id -> wanted sysctl value |
| `hardening.sysctl_file` | `/etc/sysctl.d/60-horcrux-hardening.conf` | the managed drop-in |
| `hardening.schedule_hours` | `24` | hours between scheduled checks; 0 = on demand only |
| `hardening.notify_on_regression` | `true` | notify when a rule starts failing |

## API

| Method and path | Purpose | Privilege |
|---|---|---|
| `GET /api/security/overview` | counts for the overview | Sys.Audit |
| `GET /api/security/settings`, `PUT` | read / change the settings (400 with the reason when invalid) | Sys.Audit / Sys.Modify |
| `GET /api/security/targets` | images and containers with their newest scan | Sys.Audit |
| `GET /api/security/scans?kind=&target=&limit=` | scans, newest first | Sys.Audit |
| `POST /api/security/scans` `{kind, target}` | start a scan (202; 424 `scanner-missing` / `scanning-disabled`) | Sys.Modify |
| `POST /api/security/scans/all` | scan every target (202) | Sys.Modify |
| `GET /api/security/scans/:id`, `DELETE` | one scan with its findings / remove it (409 while running) | Sys.Audit / Sys.Modify |
| `GET /api/security/hardening` | check the baseline now | Sys.Audit |
| `GET /api/security/hardening/report?format=html\|text\|json` | report download | Sys.Audit |
| `POST /api/security/hardening/apply` `{rules?, dry_run?}` | apply the named (or every failing automatic) rule | Sys.Modify |
| `POST /api/security/hardening/revert` `{rules?, dry_run?}` | revert the named (or every applied) rule | Sys.Modify |

## Not planned: guest introspection

Reading a running guest's memory from the host (hidden processes, kernel hooks) was prototyped and measured; it is deferred because it needs per-kernel symbol profiles and a patched kernel interface that distributions do not ship. Details and numbers: docs/design/security-suite.md, "Phase 4 spike".
