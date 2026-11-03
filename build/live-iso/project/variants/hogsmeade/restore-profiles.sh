#!/bin/bash
# hogsmeade-restore-profiles.sh (runs on CRYSTALCAVE): restore the owner's Windows-side browser/app profiles and the WSL dotfiles into
# /home/canutethegreat of the freshly installed HOGSMEADE BEFORE his first login (so Brave & co. never create a fresh profile + keyring first).
#   hogsmeade-restore-profiles.sh [--test <standin-dir>] -- root@host
# Source: restic repo ctg-backups/HOGSMEADE, snapshot tag pre-horcrux-wipe-profiles (Windows AppData, taken through a VSS shadow copy by a hidden
# SYSTEM task) + snapshot pre-horcrux-wipe (wsl/dotfiles.tar.zst, wsl/secrets.tar.zst). Credentials are read from files, used through env only.
#
# WHAT SURVIVES (and what does NOT) -- the Windows secrets are protected with DPAPI (key bound to the Windows login), which Linux cannot read:
#   restored as is : Brave bookmarks/history/favicons/top sites/shortcuts/extension settings; Firefox profile (NSS, not DPAPI: cookies + sessions
#                    survive); VS Code user settings; ~/.claude; .gitconfig; WSL dotfiles + non-ssh secrets (aws/gcloud/docker configs, WSL chrome profile)
#   NOT restorable : Brave/Edge cookies + saved logins + autofill secrets ("v10/v20" blobs), Brave extension sessions (Proton Pass), Nextcloud client
#                    app password (Windows Credential Manager), Discord token, Signal keys (relink), Proton Pass/Proton VPN desktop sessions.
#                    -> sign in ONCE on Linux. From then on Brave keeps its key in the GNOME keyring (gnome-libsecret store), the keyring is in the
#                    daily backup (horcrux-backup paths) and in `hogsmeade-backup`, so a later reinstall restores them.
# Never overwrites: a path that already exists on the target is skipped (and reported); refuses when the user already has a session.
set -eu
umask 077
export PATH=/usr/local/bin:/usr/bin:/bin
USERNAME=canutethegreat
HOMEDIR=/home/$USERNAME
TEST=""
if [ "${1:-}" = "--test" ]; then TEST=$2; shift 2; fi
[ "${1:-}" = "--" ] && shift
TARGET=${1:-}; [ -n "$TARGET" ] || [ -n "${HSM_STAGE_ONLY:-}" ] || { echo "usage: $0 [--test standin-dir] -- root@host" >&2; exit 2; }
SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new "$TARGET")
W=$(mktemp -d "${TMPDIR:-/tmp}/hsm-restore.XXXXXX")
trap 'chmod -R u+rwX "$W" 2>/dev/null; rm -rf "$W"' EXIT
log() { printf '%s\n' "$*" >&2; }

if [ -n "$TEST" ]; then
  cp -a "$TEST"/. "$W/src/" 2>/dev/null || { mkdir -p "$W/src"; cp -a "$TEST"/. "$W/src/"; }
else
  B2=$HOME/.hermes/state/b2-restic-hogsmeade.json
  export AWS_ACCESS_KEY_ID=$(python3 -c "import json,sys;print(json.load(open('$B2'))['applicationKeyId'])")
  export AWS_SECRET_ACCESS_KEY=$(python3 -c "import json,sys;print(json.load(open('$B2'))['applicationKey'])")
  export RESTIC_REPOSITORY=s3:https://s3.us-west-002.backblazeb2.com/ctg-backups/HOGSMEADE
  export RESTIC_PASSWORD_FILE=$HOME/.hermes/secrets/hogsmeade-restic-repo-password.txt
  mkdir -p "$W/win" "$W/wsl"
  AD=/C/Users/canut/AppData
  log "== restic restore: Windows profiles (allow-list)"
  BRD="$AD/Local/BraveSoftware/Brave-Browser/User Data/Default"
  restic restore latest --tag pre-horcrux-wipe-profiles --target "$W/win" \
    --include "$BRD/Bookmarks" --include "$BRD/History" --include "$BRD/Favicons" --include "$BRD/Top Sites" --include "$BRD/Shortcuts" \
    --include "$BRD/Local Extension Settings" --include "$BRD/Preferences" --include "$BRD/Secure Preferences" \
    --include "$AD/Roaming/Mozilla/Firefox" --include "$AD/Roaming/Code/User" \
    --include /C/Users/canut/.claude --include /C/Users/canut/.gitconfig >/dev/null
  log "== restic restore: WSL dotfiles"
  restic restore latest --tag pre-horcrux-wipe --target "$W/wsl" \
    --include /HOGSMEADE-pre-wipe/wsl/dotfiles.tar.zst --include /HOGSMEADE-pre-wipe/wsl/secrets.tar.zst >/dev/null
  chmod -R u+rwX "$W/win" "$W/wsl"       # restic restores the Windows ACL-derived modes
  mkdir -p "$W/src"
  WIN=$W/win/C/Users/canut
  BR="$WIN/AppData/Local/BraveSoftware/Brave-Browser/User Data/Default"
  if [ -d "$BR" ]; then
    d=$W/src/.config/BraveSoftware/Brave-Browser/Default; mkdir -p "$d"
    for f in Bookmarks History Favicons "Top Sites" Shortcuts "Local Extension Settings"; do [ -e "$BR/$f" ] && cp -a "$BR/$f" "$d/"; done
    # Preferences carry per-machine integrity MACs: kept as a reference (extension list etc.), not installed over Brave's own
    mkdir -p "$W/src/.local/share/hogsmeade-restore"
    python3 - "$BR/Secure Preferences" "$BR/Preferences" > "$W/src/.local/share/hogsmeade-restore/brave-windows-extensions.txt" <<'PY' || true
import json,sys
seen=set()
for f in sys.argv[1:]:
    try: p=json.load(open(f,encoding='utf-8-sig'))
    except Exception: continue
    for k,v in ((p.get('extensions',{}).get('settings',{}) or {}).items()):
        m=v.get('manifest') or {}
        if m.get('name') and k not in seen and v.get('location') in (1,4) and not m['name'].startswith('__MSG'):
            seen.add(k); print(m['name'],'|',k,'|',m.get('version'))
PY
  fi
  FF="$WIN/AppData/Roaming/Mozilla/Firefox"
  if [ -d "$FF/Profiles" ]; then
    d=$W/src/.mozilla/firefox; mkdir -p "$d"; cp -a "$FF/Profiles/." "$d/"
    prof=$(ls "$FF/Profiles" | grep -E 'default-release$' | head -1)
    [ -n "$prof" ] && printf '[Profile0]\nName=default-release\nIsRelative=1\nPath=%s\nDefault=1\n\n[General]\nStartWithLastProfile=1\nVersion=2\n' "$prof" > "$d/profiles.ini"
    rm -f "$d"/*/lock "$d"/*/.parentlock
  fi
  [ -d "$WIN/AppData/Roaming/Code/User" ] && { mkdir -p "$W/src/.config/Code"; cp -a "$WIN/AppData/Roaming/Code/User" "$W/src/.config/Code/"; }
  [ -d "$WIN/.claude" ] && cp -a "$WIN/.claude" "$W/src/.claude"
  [ -f "$WIN/.gitconfig" ] && cp -a "$WIN/.gitconfig" "$W/src/.gitconfig"
  # WSL dotfiles; secrets without .ssh (the owner's ssh keys arrive with the Nextcloud sync, ~/.ssh is theirs)
  zstd -dc "$W/wsl/HOGSMEADE-pre-wipe/wsl/dotfiles.tar.zst" | tar -x -C "$W/src" --keep-old-files 2>/dev/null || true
  zstd -dc "$W/wsl/HOGSMEADE-pre-wipe/wsl/secrets.tar.zst" | tar -x -C "$W/src" --exclude=./.ssh --exclude='./.ssh/*' --keep-old-files 2>/dev/null || true
  rm -rf "$W/win" "$W/wsl"
fi
find "$W/src" -type f | wc -l | sed 's/^/files staged: /' >&2

if [ -n "${HSM_STAGE_ONLY:-}" ]; then mkdir -p "$HSM_STAGE_ONLY" && cp -a "$W/src/." "$HSM_STAGE_ONLY/" && log "staged only -> $HSM_STAGE_ONLY"; exit 0; fi
log "== target checks"
"${SSH[@]}" "id $USERNAME >/dev/null && ! loginctl list-sessions --no-legend 2>/dev/null | awk '{print \$3}' | grep -qx $USERNAME" \
  || { log "STOP: user missing or already has a session: restore only before the first login"; exit 3; }
"${SSH[@]}" "mountpoint -q $HOMEDIR || findmnt -n $HOMEDIR >/dev/null" || log "warning: $HOMEDIR is not its own mount (check the home dataset)"
log "== push (tar over ssh stdin; existing paths are kept)"
tar -C "$W/src" -cf - . | "${SSH[@]}" "set -e; umask 077; mkdir -p $HOMEDIR; tar -x -C $HOMEDIR --keep-old-files --no-same-owner 2>/tmp/hsm-restore.skipped || true
  chown -R $USERNAME:$USERNAME $HOMEDIR/.config $HOMEDIR/.mozilla $HOMEDIR/.claude $HOMEDIR/.gitconfig $HOMEDIR/.local $HOMEDIR/.aws.old $HOMEDIR/.docker $HOMEDIR/.pki 2>/dev/null || true
  chown -R $USERNAME:$USERNAME $HOMEDIR 2>/dev/null
  chmod -R go-rwx $HOMEDIR/.config $HOMEDIR/.mozilla $HOMEDIR/.local $HOMEDIR/.claude $HOMEDIR/.gitconfig 2>/dev/null || true
  install -d -m 0700 -o $USERNAME -g $USERNAME $HOMEDIR/Nextcloud
  mkdir -p /var/lib/hogsmeade; date -u > /var/lib/hogsmeade/profiles-restored
  echo restored; echo skipped-existing: \$(wc -l < /tmp/hsm-restore.skipped)"
log "== done: tell the owner which sign-ins to redo once (see CONVERSION.md)"
