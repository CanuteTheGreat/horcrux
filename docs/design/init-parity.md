# Init parity: OpenRC and systemd, both first-class

Owner rule (2026-11-13): "horcrux and patronus should fully, 100% support openrc and systemd. gentoo is about choice."
Before this change Horcrux was OpenRC-only in practice. This is the audit, what phase 1 landed, and the ordered plan.

## Gap table

| component | OpenRC | systemd | work needed |
|---|---|---|---|
| horcrux-api service mgmt (system-admin UI, `api/system_admin/services.rs`) | ok | had its own detection | **done**: uses `horcrux_api::init` |
| cluster join boot-enable (`api/cluster_join.rs`) | rc-update only | none | **done**: `init::backend().enable()` |
| other Rust call sites (`ha_fence_agent.rs`, `guest_state/agent.rs`, `horcrux-cluster/corosync.rs`, `horcrux-nas/shares/ftp.rs`) | rc-service direct | systemctl direct, each with private detection | phase 2: move onto `init` (trait must move to `horcrux-common` for the crates) |
| ebuilds horcrux-bin / horcrux (api, guests, ha-fence) | initd | **done**: `horcrux-api.service`, `horcrux-guests.service`, `horcrux-ha-fence.service` | start the units in a systemd VM (phase 3) |
| machine layer: firstboot, timesync, touchbar (+ shared script), t2fanrd, tiny-dfr, awccd, comfyui, horcrux-test-machine | initd | **done**: unit beside every initd, `|| ( openrc systemd )` RDEPEND, revbump | runtime test on real hardware where possible |
| `app-misc/hogsmeade-tools` | initd | none | phase 2; exempt in the gate (another agent owns the GNOME-on-OpenRC fix) |
| parity gate | none | none | **done**: `gentoo/ci/check-init-parity.py` (initd without unit fails; exempt list may only shrink) |
| live ISO rootfs (`setup-rootfs.sh`, `Dockerfile.rootfs` stage3 `amd64-openrc`, `rc-update`, `/etc/init.d/live-*`, zram-swap, live-serial) | ok | **missing** | phase 3: units for the live-* services, `STAGE3_TAG=amd64-systemd` rootfs path, `INIT=systemd` in `build.sh` (image naming already supports `-systemd-`) |
| installer (`liveinst`, `project/overlay`) init choice | assumes openrc (runlevel adds, `/etc/inittab`, agetty autologin) | missing | phase 3: write `systemctl enable` presets / getty drop-ins when the target is systemd |
| QEMU test (`qemu-test.py`) | ok | **done (harness)**: `--init openrc\|systemd`, `init_cmds.py` unit-tested; `ci-run.sh` passes `$INIT` | a systemd ISO to point it at (phase 3) |
| CI workflows | `live-iso-{server,gnome-light}-openrc-*` | none | phase 3: `live-iso-server-systemd-x86_64.yml` once the ISO builds |
| binhost | amd64+arm64 openrc and systemd lanes already build | amd64/systemd was unhealthy | **fixed in gentoo-binhost 4284e6c** (below) |
| docs | OpenRC-centric | thin | phase 4: install docs per init |

## Phase 1
- `horcrux-api/src/init.rs`: `InitBackend` trait (start/stop/restart/enable/disable/state/is_enabled/exists), `OpenRcBackend`, `SystemdBackend`, runtime `detect()` (systemd iff `/run/systemd/system` is a directory and `systemctl` exists, else OpenRC). Commands go through a `CommandRunner`, so both backends are unit-tested with a recording fake.
- A systemd unit beside every OpenRC script shipped by Horcrux's ebuilds, same behaviour (same `/etc/conf.d` env file where the script has one); shared script where the initd held real logic (touchbar).
- Binhost: amd64/systemd tier 35 timed out three runs because `evolution` (-> webkit-gtk, hours) sat in it, starving tiers 36-41 and 90 (neovim, pahole, linux-firmware, gentoo-xcursors, nodejs never reached). Evolution moved to heavy tier `91-heavy-gnome-mail`; the heavy-only continuation chains while a run finishes a heavy tier. No new tier: the four lanes already exist. samba/avahi/bind per-lane coverage is documented in gentoo-binhost `docs/OPERATIONS.md` (official-binhost binaries count as covered; bind is never built in any lane).

## Ordered plan
1. Phase 2: remaining Rust call sites onto `init`; hogsmeade-tools unit after its OpenRC fix lands; exempt list to empty.
2. Phase 3: systemd live ISO, server first (live-* units, stage3 systemd rootfs, installer), then `live-iso-server-systemd-x86_64.yml` on rw-man-01 (KVM, one ISO build at a time; the aarch64 TCG lane stays OpenRC until x86 is green); GNOME/XFCE after.
3. Phase 4: docs and an INIT dimension in every edition's matrix.
