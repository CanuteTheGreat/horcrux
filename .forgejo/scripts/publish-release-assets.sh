#!/usr/bin/env bash
# Attach files to the Forgejo release of the tag being built, creating the
# release (with a changelog since the previous tag) if it doesn't exist yet.
# Several jobs run this in parallel, so a lost create race just re-reads.
#
#   FORGEJO_TOKEN=... .forgejo/scripts/publish-release-assets.sh FILE...
#
# Replaces the workflows' former actions/upload-artifact + download-artifact
# + softprops/action-gh-release chain: this instance's artifact storage
# fails every upload (see docs.yml), and action-gh-release speaks GitHub's
# release API, not Forgejo's.
#
# Env: FORGEJO_TOKEN (the job's automatic token), GITHUB_SERVER_URL,
# GITHUB_REPOSITORY, GITHUB_REF_NAME (set by the runner).
set -euo pipefail

[ $# -gt 0 ] || { echo "usage: $0 FILE..." >&2; exit 2; }
: "${FORGEJO_TOKEN:?FORGEJO_TOKEN is required}"
tag="${GITHUB_REF_NAME:?}"
api="${GITHUB_SERVER_URL:?}/api/v1/repos/${GITHUB_REPOSITORY:?}"
auth=(-H "Authorization: token ${FORGEJO_TOKEN}")

release_id() {
    curl -fsS "${auth[@]}" "$api/releases/tags/$tag" 2>/dev/null |
        python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' 2>/dev/null || true
}

id="$(release_id)"
if [ -z "$id" ]; then
    prev="$(git describe --tags --abbrev=0 "${tag}^" 2>/dev/null || true)"
    {
        echo "## Changes"
        if [ -n "$prev" ]; then
            git log --pretty=format:'- %s' "${prev}..${tag}"
        else
            echo "- Initial release"
        fi
    } >/tmp/release-notes.md
    case "$tag" in *alpha* | *beta* | *rc*) pre=true ;; *) pre=false ;; esac
    body="$(python3 -c 'import json,sys; print(json.dumps({"tag_name": sys.argv[1], "name": sys.argv[1], "body": open("/tmp/release-notes.md").read(), "prerelease": sys.argv[2] == "true"}))' "$tag" "$pre")"
    curl -fsS "${auth[@]}" -H 'Content-Type: application/json' -d "$body" \
        "$api/releases" >/dev/null ||
        echo "release create failed (another job may have created it) - re-reading"
    id="$(release_id)"
fi
[ -n "$id" ] || { echo "no release for $tag" >&2; exit 1; }

for f in "$@"; do
    name="$(basename "$f")"
    # Re-runs replace an asset of the same name.
    old="$(curl -fsS "${auth[@]}" "$api/releases/$id/assets" |
        python3 -c 'import json,sys; print(" ".join(str(a["id"]) for a in json.load(sys.stdin) if a["name"] == sys.argv[1]))' "$name")"
    for a in $old; do
        curl -fsS -X DELETE "${auth[@]}" "$api/releases/$id/assets/$a"
    done
    curl -fsS "${auth[@]}" -F "attachment=@${f}" \
        "$api/releases/$id/assets?name=${name}" >/dev/null
    echo "attached $name to release $tag ($id)"
done
