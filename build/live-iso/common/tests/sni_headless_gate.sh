#!/bin/bash
# Regression test: gnome-shell-extension-appindicator must accept a Chromium/Electron 43+
# StatusNotifierItem whose Properties.Get fails for properties it does not set
# (IconPixmap, XAyatana*). Runs a throwaway headless gnome-shell in its own private
# D-Bus session (no window, never touches the logged-in session) and a fake SNI item.
# Usage: sni_headless_gate.sh [extension-dir] [early]   ('early': the app is already running at shell start, found by the brute-force scan)   (default: the installed system extension)
# Exit: 0 pass, 1 item dropped (the bug), 77 skipped (no gnome-shell / headless unsupported)
set -u
here=$(cd "$(dirname "$0")" && pwd)
UUID=appindicatorsupport@rgcjonas.gmail.com
ext=${1:-}
mode=${2:-late}
command -v gnome-shell >/dev/null && command -v dbus-run-session >/dev/null || { echo "SKIP: no gnome-shell"; exit 77; }
work=$(mktemp -d "${TMPDIR:-/tmp}/sni-headless.XXXXXX"); trap 'chmod -R u+rwX "$work" 2>/dev/null; rm -rf "$work"' EXIT
export XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config XDG_CACHE_HOME=$work/cache XDG_RUNTIME_DIR=$work/run
mkdir -p "$XDG_DATA_HOME/gnome-shell/extensions" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
if [ -n "$ext" ]; then
    cp -r "$ext" "$XDG_DATA_HOME/gnome-shell/extensions/$UUID"
    [ -d "$ext/schemas" ] && glib-compile-schemas "$XDG_DATA_HOME/gnome-shell/extensions/$UUID/schemas"
fi
export GSETTINGS_BACKEND=keyfile   # private settings, enabled-extensions set below
cat >"$work/inner.sh" <<EOF
gsettings set org.gnome.shell enabled-extensions "['$UUID']"
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.shell disable-extension-version-validation true
gnome-shell --headless --wayland --no-x11 --virtual-monitor 800x600 >"$work/shell.log" 2>&1 &
sp=\$!
if [ "$mode" = early ]; then python3 "$here/../sni_chromium_fake.py" 15 --early >"$work/fake.log" 2>&1 & fp=\$!; fi
for i in \$(seq 1 40); do busctl --user list 2>/dev/null | grep -q org.kde.StatusNotifierWatcher && break; sleep 1; done
busctl --user list 2>/dev/null | grep -q org.kde.StatusNotifierWatcher || { echo "SKIP: shell/extension did not start"; tail -20 "$work/shell.log"; kill \$sp; exit 77; }
[ "$mode" = early ] || sleep 10   # let the extensions startup brute-force bus scan finish: only then is this a normal late registration (like Slack starting after login)
if [ "$mode" = early ]; then wait \$fp; rc=\$?; grep -E "^(registered|Get |watcher|org\\.|call )" "$work/fake.log"
else python3 "$here/../sni_chromium_fake.py" 12 2>&1 | grep -E "^(registered|Get |watcher|org\\.|call )"; rc=\${PIPESTATUS[0]}; fi
kill \$sp 2>/dev/null; wait \$sp 2>/dev/null
if grep -q "While initalizing proxy for org.freedesktop.StatusNotifierItem" "$work/shell.log"; then
  echo "FAIL: extension errored while initialising the item:"; grep -A12 "While initalizing proxy" "$work/shell.log" | head -20; rc=1
fi
[ \$rc = 0 ] && echo "PASS: item accepted"
exit \$rc
EOF
timeout 120 dbus-run-session -- bash "$work/inner.sh"
