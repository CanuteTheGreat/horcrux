# Pasting text into a VM console

The console's **Paste** button reads the text from your browser clipboard and
**types it into the guest as key presses**, the same keys you would press by
hand. That works everywhere a keyboard works: BIOS and boot-loader prompts,
installers, disk-unlock (LUKS) prompts, login screens and any guest OS, with
nothing installed in the guest.

Setting the guest's clipboard instead (what Paste did before) only works while
a clipboard agent runs in the guest, so it silently did nothing on Windows
without the guest tools and on every pre-boot screen.

## Modes

The mode select next to the button (or `paste_mode` on the node) picks what
Paste does:

| Mode | Effect |
|---|---|
| `type` (default) | types the text as key presses |
| `clipboard` | only sets the guest clipboard (needs the agent below) |
| `both` | sets the guest clipboard and types the text |

While it types, the button reads **Cancel paste** and the status shows the
progress; cancelling stops at once and releases Shift. If the browser does not
allow reading the clipboard, a prompt asks for the text instead.

## Keyboard layout

The VM turns each key the console sends into the key of its **keyboard layout**
(Display settings → *Keyboard layout*, a QEMU keymap name such as `en-us`,
`en-gb`, `de`, `de-ch`, `fr`, `es`; blank means `en-us`; applies at the next
start). Set it to the layout the guest OS uses. Capital letters and, on
`en-us`, the shifted symbols press Shift. On other layouts only A-Z press Shift
and the keymap decides the rest. Characters outside ASCII are typed only if the
VM's layout has a key for them; the console tells you how many it could not
count on, and control characters other than Tab and line breaks are skipped.

## Settings (`/etc/horcrux/config.toml`, section `[console]`)

| Key | Default | Meaning |
|---|---|---|
| `paste_mode` | `"type"` | `type`, `clipboard` or `both` |
| `paste_delay_ms` | `20` | pause after each key (10 ms was measured to drop a Shift release on a busy host; raise it for slow guests, e.g. a BIOS) |
| `paste_line_delay_ms` | `50` | extra pause after each line break |
| `paste_max_chars` | `20000` | longer text is refused |
| `paste_confirm_chars` | `500` | longer text asks for confirmation (0 = never) |
| `paste_unicode_keysyms` | `true` | try non-Latin-1 characters with Unicode keysyms |

## What is recorded

The audit log gets one entry per paste, `paste N chars` with the mode, the
user and the VM. The pasted text itself is never logged, stored or put in a URL
(it may be a password).

## Clipboard agent (optional)

To let the console set the guest's clipboard, enable *Clipboard agent channel*
in the VM's Display settings (VNC displays; SPICE displays always have it) and
install the agent in the guest:

- Linux: `spice-vdagent`.
- Windows: the SPICE guest tools (`spice-guest-tools`) or the virtio-win guest
  tools, which include the same agent.

SPICE consoles use the viewer's own clipboard handling.
