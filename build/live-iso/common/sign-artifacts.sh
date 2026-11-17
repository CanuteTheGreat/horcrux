#!/usr/bin/env bash
# sign-artifacts.sh <file>...    detached, ASCII-armoured OpenPGP signature <file>.asc for every file (release signing key)
#   Env RELEASE_SIGNING_KEY  the armoured SECRET key (a Forgejo Actions secret); it only ever lives in a 0700 temp GNUPGHOME
#       RELEASE_PUBKEY       the PUBLIC key committed to the repo (default <repo>/keys/release-signing.asc)
# The secret key must belong to the committed public key, and every signature is verified against the committed public key
# (verify-artifacts.sh) before this exits 0. Any failure = exit 1 and no .asc is left behind: the caller publishes nothing.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PUB=${RELEASE_PUBKEY:-$(cd "$HERE/../../.." && pwd)/keys/release-signing.asc}
[ $# -ge 1 ] || { echo "usage: sign-artifacts.sh <file>..." >&2; exit 2; }
for f in "$@"; do rm -f "$f.asc"; done   # never leave an older signature next to a file this run could not sign
[ -n "${RELEASE_SIGNING_KEY:-}" ] || { echo "sign-artifacts: RELEASE_SIGNING_KEY is not set: unsigned artifacts are never published" >&2; exit 1; }
[ -s "$PUB" ] || { echo "sign-artifacts: no public key at $PUB" >&2; exit 1; }
command -v gpg >/dev/null || { echo "sign-artifacts: gpg is not installed" >&2; exit 1; }
G=$(mktemp -d "${TMPDIR:-/tmp}/relsign.XXXXXX"); chmod 700 "$G"
signed=()
cleanup() { gpgconf --homedir "$G" --kill all >/dev/null 2>&1 || true; rm -rf "$G"; }
fail() { echo "sign-artifacts: $*" >&2; for s in "${signed[@]}"; do rm -f "$s"; done; exit 1; }
trap cleanup EXIT
printf '%s\n' "$RELEASE_SIGNING_KEY" | gpg --homedir "$G" --batch --quiet --import 2>/dev/null || fail "cannot import RELEASE_SIGNING_KEY"
fpr=$(gpg --homedir "$G" --batch --with-colons --list-secret-keys 2>/dev/null | awk -F: '/^fpr/{print $10; exit}')
want=$(gpg --batch --with-colons --show-keys "$PUB" 2>/dev/null | awk -F: '/^fpr/{print $10; exit}')
[ -n "$fpr" ] || fail "RELEASE_SIGNING_KEY holds no secret key"
[ "$fpr" = "$want" ] || fail "the signing key ($fpr) is not the committed release key ($want, $PUB)"
for f in "$@"; do
  [ -f "$f" ] || fail "no such file: $f"
  rm -f "$f.asc"
  gpg --homedir "$G" --batch --yes --quiet --pinentry-mode loopback --passphrase '' --local-user "$fpr" \
      --armor --detach-sign --output "$f.asc" "$f" 2>/dev/null || fail "signing $f failed"
  signed+=("$f.asc")
done
bash "$HERE/verify-artifacts.sh" "$PUB" "$@" || fail "a fresh signature does not verify against $PUB"
echo "signed with $fpr: $*"
