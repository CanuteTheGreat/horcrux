# Horcrux desktop gaming: games at the right size on a scaled GNOME desktop

Status: implemented (games-util/horcrux-game-scale-1, Horcrux overlay). QEMU: the problem (control) and the wrapper's
decisions are proven; the wrapped-size measurement needs a GL virtual GPU and is still open (section 6). Opt-in package: any Horcrux desktop
edition that has Steam installs it (machine profile or machine layer); no edition pulls it by default.

## 1. The problem

GNOME's compositor (mutter) presents every X11 (Xwayland) window at buffer scale ceil(monitor scale) and
cannot turn that off: mutter 50 removed the `xwayland-native-scaling` experimental switch
(src/backends/meta-settings.c in 50.6 only knows `kms-modifiers` and `autoclose-xwayland`). On a 2560x1600
panel at 133 % the X screen is 3840x2400 and one X pixel is 0.67 panel pixels.

* Desktop X11 apps are fine once they are told the factor: gsd-xsettings publishes `Gdk/WindowScalingFactor`
  and `Xft.dpi` (gnome-base/gnome-session-openrc-50.0-r3+ on OpenRC; gnome-session on systemd), and GTK3, Qt,
  SDL3-based Steam and Electron follow it.
* Games never read XSETTINGS. A windowed 1280x720 game covers 853x480 panel pixels at 133 % (640x400 at
  200 %): the measured control case in section 6. Fullscreen games are the right size but render at the
  oversized X screen (3840x2400 for a 2560x1600 panel, 2.25x the pixels) and mutter scales them down
  (mutter issue #3705: blurry and slow).
* `GDK_SCALE=2` fixes Steam's own window and nothing inside a game (user report).

## 2. Options evaluated

| | (a) gamescope wrapper (chosen) | (b) Proton Wayland driver | (c) SDL Wayland for native games |
|---|---|---|---|
| Covers | every X11 game: Proton and native, any engine | Proton games only | native games that use SDL2/3 with Wayland built in |
| How it fixes size | gamescope is a Wayland client of mutter, fullscreen on the monitor, renders the game in its own Xwayland at the panel's native mode and scales the game window to fill it | winewayland surfaces are Wayland surfaces; mutter scales non-DPI-aware windows | SDL's Wayland backend; mutter scales non-high-DPI windows |
| Steam overlay / Steam Input | work (the game stays an X11 client, in gamescope's Xwayland) | do not work: the overlay has no Wayland path (steam-for-linux #8020, proton-ge-custom #166/#365: no overlay = no Steam Input) | do not work (#8020; Valve: "would not recommend forcing SDL_VIDEODRIVER=wayland until the Steam Overlay supports it", steam-runtime #487/#754) |
| Availability | gamescope 3.17.0 in ::gentoo (~amd64), binhost set 39 | `PROTON_ENABLE_WAYLAND=1` is honoured by Proton-GE/CachyOS builds; upstream Valve Proton does not enable winewayland | env var, but it also reaches the Steam client process and every runtime SDL; old SDL2 lacks a Wayland backend |
| Global without per-game setup | yes, through launch options (section 4) | env var on the Steam process: yes, but breaks Steam Input | env var on the Steam process: yes, breaks overlay, risks the Steam client itself |
| Cost | one nested compositor (a copy/scale pass per frame; the game renders at native resolution instead of the 2.25x oversized X screen) | none | none |

(b) and (c) give up Steam Input (controller remapping) and the overlay for every game, which is a regression
for a Steam library; they also leave half the library (the other API) unfixed. gamescope is the only
option that fixes every game with nothing lost, so it is the default. (b) stays a per-game choice: a launch
option of `HORCRUX_GAME_SCALE=0 PROTON_ENABLE_WAYLAND=1 %command%` with a Proton build that has winewayland.

What gamescope does with the sizes (read from 3.16.28/3.17.0 src/Backends/WaylandBackend.cpp): it binds
`wp_fractional_scale_v1`, so `-W/-H` are PHYSICAL pixels and a windowed gamescope at 1280x720 would again
be small (960x540 logical at 133 %). Fullscreen (`-f`) takes its size from the compositor's configure, i.e.
the whole monitor in physical pixels. The nested (game) size `-w/-h` defaults to the initial `-W/-H`
(1280x720 when not given), so the wrapper passes the monitor's physical mode for both.

## 3. The wrapper: horcrux-game-scale

`horcrux-game-scale %command%` (Steam) or `horcrux-game-scale GAME ARGS` (any launcher):

1. Decides at launch time, from the live session: the primary (or configured) logical monitor's current
   mode, refresh rate and scale come from `org.gnome.Mutter.DisplayConfig.GetCurrentState`; nothing is
   hard-coded and a scale change or another monitor is picked up at the next launch.
2. Wraps in `gamescope --backend wayland -W <mode w> -H <mode h> -w <render w> -h <render h> -r <refresh> -f
   [-F filter] [--prefer-vk-device V:D] [extra] -- <the whole Steam command>`: gamescope sits OUTSIDE the
   Steam Linux Runtime container and Proton, on the host, so it is the same for Proton and native games.
3. Runs the game directly when nothing needs fixing: monitor scale 1 (mode `auto`), an X11 session, already
   inside gamescope (`GAMESCOPE_WAYLAND_DISPLAY`, a Steam Deck-style session, or a launch option that already
   runs gamescope), `mode = "off"`, `HORCRUX_GAME_SCALE=0`, an excluded app id, gamescope missing, no mutter.
4. Never leaves the user without the game: gamescope runs as a child; a non-zero exit within
   `fallback_seconds` (it could not start on this GPU/compositor) runs the game directly. Every decision is
   one line in `$XDG_STATE_HOME/horcrux/game-scale.log`; `horcrux-game-scale --explain -- CMD` prints it as
   JSON without starting anything.
5. Hybrid GPUs: `vk_device = "auto"` makes gamescope composite on the NVIDIA GPU when the game is
   PRIME-offloaded to it (`__NV_PRIME_RENDER_OFFLOAD=1`), so frames are not copied between GPUs.

Settings (`/etc/horcrux/game-scale.toml`, per user `~/.config/horcrux/game-scale.toml`, every key has a
documented default in the shipped file): `mode` (auto|gamescope|off), `gamescope`, `backend`, `fullscreen`,
`render` (native | WxH | factor such as 0.75), `filter`, `grab_cursor`, `vk_device`, `monitor`,
`extra_args`, `exclude_appids`, `fallback_seconds`, `log`, `log_max_bytes`; `[launch_options]` `manage`,
`wrapper`, `exclude_appids`, `interval_seconds`, `steam_dirs`.

## 4. No per-game setup: Steam launch options, managed

* Steam has no global launch option (steam-for-linux #6443, open since 2019).
* A compatibility tool cannot do it: a tool's `require_tool_appid` is the OUTER command (Proton requires the
  Steam Linux Runtime, which wraps Proton), so a tool requiring Proton would run INSIDE Proton, and a tool
  in front of the runtime would have to re-implement Proton/runtime selection (what SteamTinkerLaunch does),
  replacing the user's per-game Proton choice. The global default compatibility tool also never applies
  to native Linux games.
* The per-game `LaunchOptions` in `userdata/<account>/config/localconfig.vdf` reach both kinds of game with
  the user's own Proton choice intact. `horcrux-steam-launch-options` keeps `horcrux-game-scale %command%`
  in front of every installed game's options (existing options kept: `-novid` -> `horcrux-game-scale
  %command% -novid`, `VAR=1 %command%` -> `VAR=1 horcrux-game-scale %command%`; an option that already runs
  gamescope is left alone; tools such as Proton/runtimes, which carry a `toolmanifest.vdf`, are skipped).
  Steam reads the file at start and rewrites it at exit, so the tool edits it only while Steam is not
  running (backup kept; `remove` restores the exact previous options from its state file). A session
  autostart entry runs `horcrux-steam-launch-options watch`, which applies whenever Steam is closed and its
  config or library changed (a game installed while Steam ran is covered after Steam's next exit). The
  autostart entry is init-neutral: GNOME starts it on OpenRC and systemd alike.

## 5. Packaging

* `games-util/horcrux-game-scale` (Horcrux overlay, amd64): the wrapper, the launch-option tool, the
  config, the autostart entry; RDEPEND `gui-wm/gamescope` + pygobject. Opt-in: pulled by a machine's
  Steam package/profile, never by an edition.
* `gui-wm/gamescope` is `~amd64` on ::gentoo: the keyword is owned by sys-apps/horcrux-repo-config
  (`package.accept_keywords/horcrux-roles`) so it is in place before the package resolves; the binpkg is in
  binhost set 39 (amd64/gnome lane, the GNOME ISO profile's USE).
* Both GNOME editions (gnome-light, gnome) and both init systems: nothing in the package depends on the
  init system.

## 6. Proof (QEMU)

`tests/steam-game-scale-vm-test.py` on an installed gnome-light (OpenRC) VM with a 2560x1600 virtio-vga
display, mesa lavapipe for Vulkan (no GPU in the VM), gamescope 3.17.0. No Steam login exists for a VM, so
the test uses Steam's real launch-command shape with shell stand-ins (`reaper SteamLaunch AppId=N --
steam-launch-wrapper -- <launch option with %command% = Steam Linux Runtime entry point -- proton
waitforexitandrun game.exe>`); the launch option is the one the real horcrux-steam-launch-options wrote.
The "game" is a fixed 1280x720 X11 window that ignores XSETTINGS, measured in QMP screendumps:

Measured so far (gnome-light OpenRC, mutter 50.6, virtio-vga 2D + lavapipe, 2026-11-25):

| scale (mutter) | control: game covers | expected for the scaled desktop | wrapper decision |
|---|---|---|---|
| 1.25 | 799x449 | 1600x900 | wrap, gamescope -W 2560 -H 1600 -w 2560 -h 1600 -f |
| 1.333 | 853x479 | 1707x960 | wrap, same (from mutter's mode) |
| 1.5 requested | mutter offers 1.333 for this mode | | |
| 2.0 | 1279x719 | 2560x1440 | wrap |
| 1.0 | | | game run directly |

On a 2D virtual GPU with lavapipe, mutter rejects gamescope's buffers (zwp_linux_buffer_params_v1 error 7,
invalid_wl_buffer: mutter renders in software and cannot import them) and gamescope aborts within 0.6 s; the
wrapper's fallback then ran the game directly, as designed. The wrapped measurement therefore runs on a
GL virtual GPU (virtio-vga-gl with Venus Vulkan; gamescope starts and runs there), captured with `--vnc` or
the compositor's own ScreenCast (tests/data/mutter-monitor-capture.py) because QMP screendump cannot read a
GL scanout.

## 7. Limits

* Proven with lavapipe in a VM, not on NVIDIA hardware: nested gamescope on the NVIDIA proprietary driver
  (and on hybrid Intel+NVIDIA laptops) has known rough edges upstream. The fallback (section 3.4) keeps
  every game playable if gamescope cannot start; a game that misbehaves inside gamescope gets
  `HORCRUX_GAME_SCALE=0 %command%` or an `exclude_appids` entry.
* gamescope is fullscreen by default: a game the user wants windowed on the desktop uses `fullscreen =
  false` (a monitor-sized window) or is excluded.
* HDR, VRR and the Steam overlay's own scale are outside this change (gamescope supports HDR/VRR flags
  through `extra_args`).
* VR titles are not wrapped by default only when listed in `exclude_appids`; add SteamVR games there.
