# gentoo/surface - Microsoft Surface hardware profile

Same structure as the other hardware profiles (`gentoo/<profile>/`): data the ISO builder and the installer read. Reasoning, sources
and the verified/device-only split are in **[docs/SURFACE.md](../../docs/SURFACE.md)**. Owner steps: ctg-infrastructure `hosts/surface-pro/CONVERSION.md`.

| path | what |
|---|---|
| `hwprofile.toml` | DMI match (Microsoft Corporation + `Surface with Windows 8 Pro` / `Surface Pro` / `Surface Pro 2` = model `surface-pro-1`; any other `Surface*` = generic), defaults (XFS+LUKS, desktop, no serial console), kernel quirk `button.lid_init_state=open`, services. Installed as `/usr/share/live-installer/hwprofiles/surface.toml`. |
| `packages.txt`, `portage/`, `portage-local/` | the desktop (sway, foot, wmenu, wvkbd, NetworkManager, PipeWire, elogind). Binpkg-first; `portage-local` is a local overlay for what `::gentoo` lacks (wvkbd). |
| `overlay/` | `/etc/sway/config` (touch-sized bar, OSK toggle), `surface-session`/`profile.d` (console login -> sway; opt out with `~/.no-desktop`), `surface-hidpi` (EDID -> scale, touch/pen mapping), `surface-status`, `surface-osk`, `surface-brightness`, OpenRC `zram-swap`, NetworkManager `wifi.powersave=2`, sysctl for zram, `horcrux-surface-report` (facts for the remote hand-off). |
| `kernel/` | **optional** linux-surface kernel for SP3+ (pin, `build-surface-kernel.sh`, config fragment). Not used for the SP1/SP2. |
| `tests/surface-qemu-test.py` | end-to-end QEMU test (IvyBridge, 4 GiB, UEFI, USB-only keyboard at the LUKS prompt). |

## Build the Surface ISO

```sh
HW_PROFILE=surface build/live-iso/build.sh -o OUT          # = the Horcrux ISO + build/live-iso/common/Dockerfile.profile layer
# -> OUT/horcrux-surface-<date>-x86_64.iso
```
Run it on an rw-man host inside `agent-workdir run`. The profile layer is its own Docker layer: editing `overlay/` never redoes the product emerge.

## Test

```sh
python3 gentoo/surface/tests/surface-qemu-test.py --iso OUT/horcrux-surface-*.iso --out EVIDENCE   # on a KVM host (rw-man-01)
python3 -m unittest discover -s build/live-iso/common/tests                                         # unit tests (hwprofile, luksroot)
```

## Adding another machine

Copy this directory, change `hwprofile.toml` (`[match]`, `[[models]]`), `packages.txt`, `overlay/`; build with `HW_PROFILE=<dir name>`.
The installer needs no code change: `liveinst/hwprofile.py` reads every `*.toml` in `/usr/share/live-installer/hwprofiles/`.
