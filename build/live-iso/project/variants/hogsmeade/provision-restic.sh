#!/bin/bash
# hogsmeade-provision-restic.sh : run on crystalcave AFTER HOGSMEADE's first boot. Pushes the restic B2 credentials and repo password
# over SSH, through STDIN, into root-only 0600 files on the target (the same way the gringotts restore push works).
# The ISO never contains these secrets; nothing is printed, nothing is passed on a command line.
#
#   hogsmeade-provision-restic.sh [--test] -- <ssh args...>        e.g.  -- root@hogsmeade   or   -- -p 2222 -i key root@127.0.0.1
#   --test : throwaway local repo (/var/lib/hsm-test-restic) + random password, no real secret is read (used by the QEMU gate)
set -euo pipefail
umask 077
TEST=0
[ "${1:-}" = "--test" ] && { TEST=1; shift; }
[ "${1:-}" = "--" ] && shift
[ $# -ge 1 ] || { echo "usage: $0 [--test] -- <ssh args>" >&2; exit 2; }
# shellcheck disable=SC2206
SSH=(ssh -o BatchMode=yes ${HSM_SSH_OPTS:--o StrictHostKeyChecking=accept-new} "$@")
H=${HOME}
if [ $TEST = 1 ]; then
    PW=$(head -c 24 /dev/urandom | base64)
    ENVTXT=$'RESTIC_REPOSITORY=/var/lib/hsm-test-restic\nRESTIC_PASSWORD_FILE=/etc/hogsmeade/restic-password\n'
else
    PW=$(cat "$H/.hermes/secrets/hogsmeade-restic-repo-password.txt")
    ENVTXT=$(python3 - "$H" <<'PY'
import json, sys
b = json.load(open(sys.argv[1] + '/.hermes/state/b2-restic-hogsmeade.json'))
print("RESTIC_REPOSITORY=s3:https://s3.us-west-002.backblazeb2.com/ctg-backups/HOGSMEADE\nRESTIC_PASSWORD_FILE=/etc/hogsmeade/restic-password\n"
      f"AWS_ACCESS_KEY_ID={b['applicationKeyId']}\nAWS_SECRET_ACCESS_KEY={b['applicationKey']}")
PY
)
fi
"${SSH[@]}" 'umask 077; install -d -m 0700 -o root -g root /etc/hogsmeade' </dev/null
printf '%s' "$PW"      | "${SSH[@]}" 'umask 077; cat > /etc/hogsmeade/restic-password.new && chmod 0600 /etc/hogsmeade/restic-password.new && mv -f /etc/hogsmeade/restic-password.new /etc/hogsmeade/restic-password'
printf '%s\n' "$ENVTXT" | "${SSH[@]}" 'umask 077; cat > /etc/hogsmeade/backup.env.new && chmod 0600 /etc/hogsmeade/backup.env.new && mv -f /etc/hogsmeade/backup.env.new /etc/hogsmeade/backup.env'
unset PW ENVTXT
# prove it works without showing anything: modes + repository reachable (test repo: initialised by the first hogsmeade-backup)
"${SSH[@]}" 'stat -c "%a %U %n" /etc/hogsmeade/backup.env /etc/hogsmeade/restic-password; set -a; . /etc/hogsmeade/backup.env; set +a; if restic cat config >/dev/null 2>&1; then echo "restic repository reachable: OK"; else echo "restic repository not reachable/initialised yet (expected for a fresh throwaway repo)"; fi' </dev/null
