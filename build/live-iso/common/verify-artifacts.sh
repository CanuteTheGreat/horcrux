#!/usr/bin/env bash
# verify-artifacts.sh <public-key.asc> <file>...   every <file> has a <file>.asc made by THAT key (gpgv, no trust database);
# prints one "GOOD <file> <fingerprint>" line per file, exit 1 on the first missing/bad signature.
set -euo pipefail
PUB=${1:?usage: verify-artifacts.sh <public-key.asc> <file>...}; shift
[ $# -ge 1 ] || { echo "usage: verify-artifacts.sh <public-key.asc> <file>..." >&2; exit 2; }
G=$(mktemp -d "${TMPDIR:-/tmp}/relverify.XXXXXX"); chmod 700 "$G"; trap 'rm -rf "$G"' EXIT
gpg --batch --quiet --dearmor < "$PUB" > "$G/release.gpg" 2>/dev/null || { echo "verify-artifacts: cannot read the public key $PUB" >&2; exit 1; }
want=$(gpg --batch --with-colons --show-keys "$PUB" 2>/dev/null | awk -F: '/^fpr/{print $10; exit}')
for f in "$@"; do
  [ -s "$f.asc" ] || { echo "verify-artifacts: $f has no signature ($f.asc)" >&2; exit 1; }
  st=$(GNUPGHOME=$G gpgv --status-fd 1 --keyring "$G/release.gpg" "$f.asc" "$f" 2>/dev/null) || true
  fp=$(printf '%s\n' "$st" | sed -n 's/^\[GNUPG:\] VALIDSIG \([0-9A-F]*\) .* \([0-9A-F]\{40\}\)$/\2/p' | head -1)
  [ -n "$fp" ] || fp=$(printf '%s\n' "$st" | sed -n 's/^\[GNUPG:\] VALIDSIG \([0-9A-F]\{40\}\).*/\1/p' | head -1)
  [ -n "$fp" ] && [ "$fp" = "$want" ] || { echo "verify-artifacts: BAD or foreign signature on $f" >&2; exit 1; }
  echo "GOOD $(basename "$f") $fp"
done
