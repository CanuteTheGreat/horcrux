# LXC containers (Proxmox `pct` parity)

Horcrux manages LXC system containers with the same feature set as Proxmox
VE's `pct`/`pveam`, on Gentoo (`USE=lxc`, pulls in `app-containers/lxc`).

## Templates (`pveam`)

| Task | UI | API | CLI |
|---|---|---|---|
| Browse the catalog (images.linuxcontainers.org, any distro/release/arch) | Containers → LXC Templates | `GET /api/lxc/templates/catalog?arch=&distro=&refresh=` | `horcrux container template available --distro debian` |
| Download (SHA-256 verified against the build's `SHA256SUMS`) | Download button | `POST /api/lxc/templates {distro, release, arch?, variant?}` | `horcrux container template download alpine/3.22` |
| List / delete cached templates | same page | `GET /api/lxc/templates`, `DELETE /api/lxc/templates/<d>/<r>/<a>/<v>` | `template list`, `template delete` |

Templates are cached in `<data_dir>/lxc-templates/<distro>/<release>/<arch>/<variant>/`.
Set `HORCRUX_LXC_IMAGES_URL` to use a mirror. A create request may name a
template that isn't downloaded yet (`alpine/3.22`, `debian/bookworm/cloud`,
`ubuntu/noble/amd64/default`); it is downloaded first. An absolute path to a
local rootfs tarball (`.tar`, `.tar.gz`, `.tar.xz`, `.tar.zst`) also works if
the file is in the image directory (`HORCRUX_IMAGE_DIR`, default
`<data_dir>/images`, the same one disk imports and ISOs use) or the template
cache. Other paths are refused (422), so a user who may create containers
can't unpack an arbitrary host archive, such as another guest's backup, into
a container they control.

## Create (`pct create`)

`POST /api/containers` with `runtime: "lxc"`, `rootfs: <template>` and an
`lxc` object (all optional):

| Field | Meaning |
|---|---|
| `unprivileged` (default `true`), `idmap_base` (100000), `idmap_size` (65536) | User namespace; root's `/etc/subuid`/`/etc/subgid` range is added if missing |
| `disk_gb` | Sized ext4 image root disk (grow later); unset = directory rootfs |
| `memory`, `swap_mb`, `cpus`, `cpu_limit` (cores, fractional), `cpu_units` | cgroup2 `memory.max`, `memory.swap.max`, `cpu.max` ("quota period"), `cpu.weight` |
| `nets[]`: `bridge`, `vlan`, `ip` (`dhcp` / CIDR), `gateway`, `ip6` (`auto`/`dhcp`/CIDR), `gateway6`, `hwaddr`, `mtu`, `name` | veth on a bridge; VLAN tags need a VLAN-aware bridge. The guest's own network config (ifupdown, netplan or systemd-networkd) is written, like Proxmox |
| `mounts[]`: `path`, `source` (host dir = bind mount), `read_only` | Without `source`: a managed volume dir owned by the container's root |
| `features`: `nesting`, `keyctl`, `fuse` | Nested containers; keyctl() allowed (blocked by default for unprivileged); `/dev/fuse` |
| `hostname`, `nameserver`, `searchdomain`, `ssh_public_keys` | Written into the guest at every start (opt out per file with `/etc/.horcrux-ignore.<file>`) |
| `onboot`, `startup_order`, `startup_delay` | `lxc.start.auto/order/delay` |

The CLI takes the same options with Proxmox-style specs:
`horcrux container create -r lxc -n web -i alpine/3.22 --disk 4 --net bridge=vmbr0,tag=20,ip=10.0.0.5/24,gw=10.0.0.1 --mp mp=/srv,source=/tank/srv,ro=1 --features nesting=1`.

Settings are stored in `/var/lib/lxc/<name>/horcrux.json`; the LXC config is
generated from them (so it survives API restarts and can't drift).

## Lifecycle and management

| `pct` | API | UI |
|---|---|---|
| start / stop (kill) | `POST /api/containers/:id/start`, `/stop` | list + Manage page |
| shutdown (clean, `timeout`, `force`) | `POST /api/containers/:id/shutdown` | Manage → Shutdown |
| reboot (applies pending settings) | `POST /api/containers/:id/reboot` | Manage → Reboot |
| suspend/resume (freeze) | `POST /api/containers/:id/pause`, `/resume` | Manage → Freeze/Thaw |
| enter / console | exec WebSocket (`lxc-attach` on a PTY), console log WebSocket | Console, Boot log |
| config / set | `GET`/`PUT /api/containers/:id/lxc/config` (limits apply live; NIC/mount/feature/hostname/DNS/key changes are listed as `pending` until reboot) | Manage → Settings |
| resize (grow, online) | `POST /api/containers/:id/resize {disk_gb}` | Manage → Root disk |
| snapshot / listsnapshot / rollback / delsnapshot | `/api/containers/:id/snapshots[...]` (a running container is frozen while copied) | Manage → Snapshots |
| clone --full | `POST /api/containers/:id/clone` (new MACs, copied volumes) | Manage → Clone |
| destroy | `DELETE /api/containers/:id` (snapshots and managed volumes too) | list → Delete |

Backups of LXC containers (vzdump equivalent) are under Backup & Protection.
Migration between nodes is not implemented yet.

## Tests

`horcrux-api/tests/lxc_e2e.sh` runs every feature above against real nested
LXC in CI (Test job, step "LXC containers"), and the UI E2E job drives the
LXC pages in a browser against a real LXC host (`lxc_ui_e2e.py`).
