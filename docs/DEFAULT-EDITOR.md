# Default editor

The system default text editor (`EDITOR` / `VISUAL` for login shells, `git commit`,
`crontab -e`, `visudo`, `sudoedit`, `systemctl edit`) is **vim** on every tier.
**neovim** is the one-setting alternative; nano is never the default.

## Setting it

| Where | How |
|---|---|
| Web UI | System > Node Configuration > General > **Default editor** (select + Save) |
| API | `GET /api/system/editor` -> `{choice, effective, installed[]}`; `PUT /api/system/editor {"choice": "neovim"}` |
| Declarative | `system: {editor: neovim}` (docs/design/declarative-config.md) |
| Shell | `default-editor set neovim` (`show`, `apply [--if-unset]`) |
| File | `/etc/default-editor.conf`: `EDITOR_CHOICE=vim\|neovim\|nano\|vi\|/absolute/path` |

Accepted values: `vim`, `neovim`, `nano`, `vi`, or a plain absolute path. Anything else
is rejected (HTTP 400, or a plan error `system.editor: ...`). A choice whose binary is not
installed falls back (vim, neovim, vi) and the API's `effective` field shows what
`EDITOR` really is. A node without `/usr/sbin/default-editor` answers `424
default-editor-missing`; the UI shows an empty state saying so. The API needs the same
rights as the other `/api/system` routes (read: audit, write: modify) and every change is
audit logged. Init-neutral: nothing here touches OpenRC or systemd.

## neovim is on every tier

`app-editors/neovim` is in `gentoo/sets/horcrux` (so the binhost builds it and installed
hosts get it with `@horcrux`) and in `build/live-iso/common/packages-extra.txt` (its own
image layer on x86_64 and aarch64, so a live/installed ISO has `nvim` and
`default-editor set neovim` works without network access).

## QA

`build/live-iso/common/headless_gates.py` `editor_gate(sh, step, user)` proves the booted
image (login-shell `EDITOR`/`VISUAL` and that `crontab -e`, `git commit`, `visudo` and
`sudoedit` start vim). `editor_gate(..., choice="neovim")` first runs `default-editor set
neovim`, requires every probe to start `nvim`, then restores vim. Static check:
`ci/check-default-editor.py`.
