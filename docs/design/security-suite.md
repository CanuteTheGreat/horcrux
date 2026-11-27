# Built-in security suite

Status: phase 1 shipped (vulnerability scanning of container images and containers, host
hardening baseline with compliance report). Later phases are planned below, one tracked job each.

Horcrux is the management plane for every guest on a node. A hypervisor that is easy to run but
hard to keep secure is a liability, so security tooling is part of the product, not a list of
packages the operator has to find, wire up and watch. Everything here uses free and open-source
tooling, is a setting (node settings, `PUT /api/security/settings`, declarative `security:`),
works the same on OpenRC and systemd, and degrades to a clear "not installed, here is how" state
(HTTP 424 with a reason code) when a tool is absent.

## What the others ship (2026-11)

| Capability | Proxmox VE 9.x | VMware vSphere 9 / VCF | Nutanix AOS/AHV | Hyper-V / Azure Local | Horcrux |
|---|---|---|---|---|---|
| Host/guest firewall | yes (nftables proxmox-firewall, VM/CT rules, security groups, IPsets) | NSX (separate product), distributed firewall | Flow Network Security (microsegmentation, licensed) | Windows Firewall, SDN ACLs | yes (nftables, security groups, firewall objects); microsegmentation: per-guest default-deny profiles, templates, group rules, hit counters, flow view (phase 3, shipped) |
| Two-factor / SSO | TOTP, WebAuthn, recovery keys, OpenID Connect, LDAP/AD | SSO, MFA through identity federation | SAML, MFA via IdP | AD, Entra | TOTP/WebAuthn, PAM, LDAP, OIDC realms |
| API / admin access log | task log + syslog, no per-request API log | vCenter events, syslog | audit log | event log | per-request API access log (`/api/access-log`), audit log |
| Host hardening baseline + report | none (docs only) | Security Configuration and Hardening Guide, compliance through Aria Operations packs | SCMA: STIG baseline checked hourly and self-healed | Secured-core, OSConfig security baseline with drift control | **Horcrux baseline (phase 1)**: CIS-derived, hypervisor-safe, apply/revert, scheduled re-check with regression alerts, HTML/text/JSON report |
| Image / guest vulnerability scanning | none | none built in (Carbon Black / third-party) | none built in | Defender for Cloud (paid, cloud) | **phase 1: container images (Podman, Docker) and container root file systems (LXC, Incus); phase 2: VMs (stopped: read-only disk mount; running: guest-agent package inventory as an SBOM)** |
| Host CVE exposure | none (apt is the tool) | Lifecycle Manager patch baselines | LCM | Windows Update / Azure Update Manager | phase 2: Gentoo GLSA + ctg-vulnwatch-style feed matching for the host |
| Host IDS / integrity | none | none built in | none built in | Defender for Endpoint (paid) | phase 2: auditd rules + AIDE, phase 3: Falco-class container runtime detection |
| Guest secrets for provisioning | cloud-init passwords stored in the config | guest customization specs | cloud-init | unattend files | phase 3: secrets store references in cloud-init/unattend, never in the VM config |
| Pentest VM templates | none | none | none | none | phase 2: Kali, Parrot, Pentoo as first-class templates |
| Guest introspection | none | NSX Guest Introspection (deprecated) | none | none | phase 4 research (optional, off by default) |

Sources: Proxmox VE admin guide (pve-docs, chapters Firewall, User Management, Two-factor
authentication); Broadcom vSphere Security Configuration Guide; Nutanix Security Guide (SCMA);
Microsoft Learn: Azure Local security baseline and drift control. Claims about other products
describe their own documentation at the time of writing.

The gap the leaders leave open: on Proxmox and XCP-ng the operator gets a firewall and 2FA and
nothing that tells him whether the node or its workloads are actually in a good state. Nutanix
SCMA is the model for the baseline (checked continuously, drift reported); none of the
self-hosted platforms scan their own workloads.

## Decisions

1. **Scanner: Grype by default, Trivy as the alternative.** Both are Apache-2.0, offline-capable
   and read Podman/Docker images and plain directories. Grype is in ::gentoo
   (`app-containers/grype`, with `syft`); Trivy is only in overlays. The scanner, its path, the
   database mirror (air-gapped nodes) and its cache directory are settings. Horcrux runs the
   scanner as a separate process: no scanner code is linked into Horcrux.
2. **Scans are resource-aware.** One scan at a time by default (`max_concurrent`), CPU niceness
   15 and the idle IO class set in the child before exec, a hard time limit, results pruned to
   `keep_scans`. The schedule (default every 24 h) derives "due" from the newest scan on record,
   so a restart never rescans everything at once.
3. **Phase 1 targets are what can be read without touching a guest**: Podman and Docker images
   (`podman:`/`docker:` sources, read through the runtime) and LXC/Incus container root file
   systems (`dir:` scan of `<lxc_path>/<name>/rootfs`, `<incus_path>/containers/<name>/rootfs`).
   Running VMs are phase 2: an offline scan of a disk needs a read-only snapshot export and must
   respect encrypted guests; a running guest is better served by the guest agent's package
   inventory. Neither is a quick add, so neither is faked now.
4. **The baseline is product content, not settings.** The rule set (`horcrux-baseline-1`, 44
   rules in kernel, network, files, ssh, accounts, mounts, hypervisor, services) ships with the
   release like a benchmark. What a node does with it is configurable: `hardening.exclude`,
   per-rule `overrides` (sysctl values), the sysctl drop-in path, schedule, notifications.
5. **Hypervisor-safe by construction.** The baseline deliberately leaves out CIS items that break
   a virtualization host: IPv4/IPv6 forwarding off (NAT and routed guest networks need it), strict
   reverse-path filtering (multi-homed and routed guests), disabling unprivileged user namespaces
   (rootless Podman, unprivileged LXC), `kernel.kexec_load_disabled` and
   `unprivileged_bpf_disabled=1` (one-way until reboot; the baseline uses the revertible value 2).
   A unit test refuses any forwarding or userns key in the rule table.
6. **Only reversible changes are automatic.** Apply changes kernel parameters (written to
   `/proc/sys` now and to `/etc/sysctl.d/60-horcrux-hardening.conf`, which OpenRC's `sysctl`
   service and `systemd-sysctl` both read at boot) and file modes (bits removed). The previous
   value of each is recorded in `<data_dir>/security/hardening-state.json`; Revert restores it and
   removes the drop-in when empty. SSH daemon, account, mount and package findings are reported
   with the exact fix and never changed by Horcrux: a wrong sshd change locks the operator out.
   Every apply is previewed (dry run) in the UI before it is confirmed.
7. **Hypervisor-specific checks first-class**: unmitigated CPU vulnerabilities
   (`/sys/devices/system/cpu/vulnerabilities`, incl. "SMT vulnerable") are a critical rule because
   they are the guest-to-guest boundary; the API access log being on is a rule.
8. **Alerts go through the notification system** as source `security` (works with the matcher
   routing of hx-notification-matchers): a finished scan with findings at or above
   `notify_min_severity` (default critical), and a scheduled baseline check that finds a rule
   newly failing.
9. **Score**: weighted share of passing rules over pass + fail + manual (critical 8, high 4,
   medium 2, else 1); not-applicable and excluded rules do not count.

## Phase 1 (shipped)

- API `horcrux-api/src/api/security/` (`mod.rs` routes + scheduler, `vulnscan.rs`,
  `hardening.rs`), shared types `horcrux-common/src/security.rs`, RBAC area `security`
  (read Sys.Audit, change Sys.Modify), declarative `security:` section.
- UI: Security > Overview / Vulnerabilities (targets, scan, scan all, recent scans, scan detail
  with severity/fix filters) / Hardening (score, filters, apply baseline, per-rule apply/revert,
  HTML/text report) / Settings. Linked from the System menu.
- Tests: unit tests for both scanner parsers, command building, target validation, the scan
  store, every baseline rule class on a fake root, apply/revert round trip, exclusions and
  overrides, report rendering, settings validation, declarative plan/export; end-to-end
  `tests/it/security_tests.rs` (real binary, stand-in scanner, fake host root).
- Packaging: the `security` USE flag on app-emulation/horcrux pulls `app-containers/grype`.

## Phase 2: VM guest scanning (shipped)

New `ScanTargetKind::Vm`. A VM is never scanned by reading its live disk:

- **Running VM** (`vuln_scan.vm_agent_scan`, default on): the Horcrux guest
  agent reports the guest's installed packages — `horcrux-guest-agent
  --inventory` (new subcommand), run through the QEMU guest-agent channel
  (`guest-exec`). Linux reads dpkg, rpm, apk or Portage; Windows reads the
  "Uninstall" registry keys. The node turns the inventory into a CycloneDX
  SBOM (`GuestInventory::to_sbom`, package PURLs + `syft:distro:*`) that the
  scanner reads with `sbom:`. The host never opens the guest file system.
- **Stopped VM** (`vuln_scan.vm_disk_scan`, default on): the first disk is
  attached read-only through `qemu-nbd --read-only` (the image is never
  written), the guest file system is mounted read-only (`ro,noexec,nosuid,
  nodev`, no journal replay) and scanned with `dir:`, then unmounted and the
  nbd export disconnected (RAII guards clean up on every path). An encrypted
  volume — `crypto_LUKS` or BitLocker, by `blkid`/`lsblk` FSTYPE or the
  on-disk magic — is skipped with a clear reason; Horcrux never unlocks a
  guest disk. For an encrypted guest, scan it running through its agent.

Settings (`VulnScanSettings`): `vm_disk_scan`, `vm_agent_scan`,
`vm_mount_timeout_secs` (10-3600, default 120); `vm` is in `schedule_kinds`
by default, so VMs are scanned on the normal schedule with the same resource
caps. VM rows appear in Security > Vulnerabilities; the VM detail page shows a
Security tab/badge. The guest SBOM format is verified against real Grype
(reports real deb CVEs with fix versions from a hand-built CycloneDX SBOM).

## Guest microsegmentation (phase 3, shipped)

Security > Microsegmentation puts a default-deny firewall around one VM or container, per NIC,
on top of the existing nftables firewall and firewall objects (IP sets, aliases, macros).
Code: `crates/horcrux-sdn/src/firewall/microseg.rs` (pure plan + command builder, parsers),
`nftables.rs` (installs it), `horcrux-api/src/api/security/microseg.rs` (API),
`horcrux-ui/src/pages/security_mseg.rs`, types in `horcrux-common/src/security.rs`.

**Model.** A profile (`security.microseg.profiles["vm:100"]`, so the declarative `security:` section
manages it) has a template, a default action per direction (deny), `allow_from` / `allow_to`
groups (an IP set `+name`, an alias or a CIDR), a lab network and a port list. `plan()` expands it into
ordinary firewall rules; the guest's stored rules stay and run first.

| Template | Inbound | Outbound |
|---|---|---|
| web server | tcp 80,443 (or `ports`) from anywhere | DNS, NTP, tcp 80/443 |
| database | tcp `ports` (3306,5432) from the listed groups only | DNS, NTP |
| isolated lab | the lab network | the lab network |
| pentest box | callbacks from the lab network, the listed groups may manage it | the lab network only (a default-allow outbound is refused by validation) |
| custom | `allow_from` | `allow_to` |

**Enforcement per NIC.** VM taps and container veths of every bridge and SDN vnet are matched by the guest's
stable device prefix in the `bridge` family (`vm-<id>-in/out`, `ct-<id>-in/out` chains jumped to from
`forward`/`input`/`output`), so a new NIC or a restarted VM is covered without a re-apply. Each direction
starts with `ct state established,related accept` and `ct state invalid drop`, then ARP, IPv6
neighbour discovery and DHCP (setting `allow_infrastructure`), then the rules, then a rate-limited log rule
(`hx-mseg: <guest>/<dir> `) and `counter drop`. Macvtap NICs are not bridge ports, so they get the same
rules as `netdev` hooks on the device (`ingress` = from the guest, `egress` = to it). Those hooks run
before connection tracking, so replies are matched statelessly (any TCP segment except a first SYN,
ICMP answers, UDP replies mirrored from the accepted UDP rules). The VM start path re-applies the
profile because the macvtap device only exists while the VM runs. IPv4 rules only, as the rest of the
firewall; IPv6 is dropped by default except neighbour discovery.

**Counters and flows.** Every rule carries `counter` and a `hx:<rule id>` comment; `nft -j list chain` gives
packets and bytes per rule (API `.../rules`, UI table). The flow view joins tracked flows (`/proc/net/nf_conntrack`,
else `conntrack -L`; optional `?ip=`) with the dropped flows read back from the kernel log by the guest's prefix.

**Verification.** `horcrux-api/tests/microseg_e2e.sh` (CI step "Guest microsegmentation") runs the real API in a privileged
container with a bridge and network namespaces: a denied flow is dropped and an allowed one passes for every
template, group membership changes take effect without an apply, counters move, stored rules coexist, macvtap-style
device filters work, and switching the feature off removes everything.

### Benchmark (public documentation, checked 2026-11-27)

| | Proxmox VE firewall | VMware NSX distributed firewall | Horcrux |
|---|---|---|---|
| Enforcement point | per-VM/CT NIC firewall flag; bridge-level iptables or, in the new `proxmox-firewall`, nftables | per vNIC at the hypervisor ("Applied To") | per guest, per NIC, nftables (bridge family; netdev for macvtap) |
| Default | per-VM IN policy, shipped examples use `IN DROP / OUT ACCEPT` | default rule (allow or deny), micro-segmentation designed as default-deny | default deny both ways, per-direction override |
| Grouping | security groups, IP sets, aliases | groups with tag, name or other dynamic membership; identity-based groups | IP sets, aliases, CIDRs (static; no tag-based membership yet) |
| Templates | none (security groups are rule bundles) | policy templates, partner and context profiles | web server, database, isolated lab, pentest box, custom |
| Rule hit counters | none in the UI (iptables/nft counters only on the CLI) | per-rule hit count in Firewall Insights | per rule, in the guest page, live |
| Flow view | conntrack logging option | traffic analysis (a separate, licensed intelligence tier) | tracked flows + dropped flows per guest |
| Layer 7 / identity | no | context profiles, identity firewall | no (gap) |
| Cost | free | licensed | free |

Sources: pve.proxmox.com/wiki/Firewall; techdocs.broadcom.com (Firewall Insights, "hit counts of the distributed
firewall rule"); kb.expedient.com ("controls traffic in and out of each vNIC individually"). Known gaps against NSX:
tag/attribute-based dynamic groups and identity or application-aware rules; guests are keyed by id, groups are address based.

## Later phases (one job each)

| Phase | Item | Notes |
|---|---|---|
| 2 | Scanner packaging in images | current Grype as `app-containers/grype-bin` in the Horcrux overlay (::gentoo has an old ~amd64 0.86), binhost tier, server + GNOME images, `horcrux-bin` optional dependency, DB mirror docs for air-gapped nodes |
| 2 | VM guest scanning | **shipped**: see "Phase 2: VM guest scanning" below |
| 2 | Host CVE exposure | Gentoo GLSA (`glsa-check`) + package versions matched against the feed ctg-vulnwatch uses; shown on the Security overview, notification on new affecting advisories, ties into Packages > updates |
| 2 | Host IDS / integrity | auditd with a Horcrux rule set (logins, privilege use, /etc changes, horcrux binaries), AIDE database init + scheduled check, findings in Security > Events and as notifications; OpenRC + systemd units |
| 2 | Pentest VM template library | Kali, Parrot, Pentoo as first-class templates (download + checksum/signature check, cloud-init or answer files where the distro supports it, sensible hardware defaults, isolated network by default) |
| 3 | Security dashboard + events | one page joining access log (hx-api-access-log), audit log, failed logins, baseline drift and scan results; alerting through hx-notification-matchers |
| 3 | Guest provisioning secrets | cloud-init/unattend passwords and keys referenced from an encrypted secrets store, never stored in VM config or exports; rotation |
| 3 | Microsegmentation templates | **shipped**: see "Guest microsegmentation" below |
| 3 | Container runtime detection | Falco-class (eBPF, Apache-2.0) rules for containers on the node, optional and resource-capped |
| 4 | Guest introspection (research) | VMI-based (LibVMI) read-only checks, off by default; go/no-go spike with measured overhead |
