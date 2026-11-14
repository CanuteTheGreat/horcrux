#!/bin/sh
# ci-already-published.sh <image-id>   (e.g. server-systemd-x86_64)
# Exit 0 when the status marker `daily-status-<image-id>` says the last daily of this image PASSED and was built from $GITHUB_SHA: a nightly
# workflow then has nothing new to build and must not occupy a runner (the systemd twins run on a nightly slot, live-iso-*-systemd-*.yml).
# Exit 1 otherwise (no marker, failed last time, older commit, API unreachable): build.
id=${1:?image id}
[ -n "${GITHUB_SERVER_URL:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ] && [ -n "${GITHUB_SHA:-}" ] || exit 1
body=$(curl -fsS --max-time 30 "$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY/releases/tags/daily-status-$id" 2>/dev/null) || exit 1
prev=$(printf '%s' "$body" | python3 -c '
import json, sys
try:
    b = json.loads(json.load(sys.stdin).get("body") or "{}")
    print(b.get("sha", "") if b.get("result") == "pass" else "")
except Exception:
    print("")' 2>/dev/null)
if [ -n "$prev" ] && [ "$prev" = "$GITHUB_SHA" ]; then
	echo "daily $id of $GITHUB_SHA is already published and passing: nothing to build"
	exit 0
fi
exit 1
