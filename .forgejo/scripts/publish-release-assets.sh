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
    # Release notes = this version's CHANGELOG.md section (ci/changelog.py; release.yml's
    # verify job already refused a tag without one). Never the raw git log: old commit
    # subjects carry lab machine names that must not be republished.
    if ! python3 ci/changelog.py --section "$tag" >/tmp/release-notes.md 2>/dev/null; then
        echo "Horcrux $tag. See CHANGELOG.md." >/tmp/release-notes.md
    fi
    {
        echo
        echo "Update and rollback: https://github.com/CanuteTheGreat/horcrux/blob/main/docs/UPDATE-POLICY.md"
        echo "Privacy (what Horcrux sends off the machine): https://github.com/CanuteTheGreat/horcrux/blob/main/docs/PRIVACY.md"
        echo "Third-party licenses: the horcrux-$tag-THIRD-PARTY-LICENSES.txt asset of this release."
    } >>/tmp/release-notes.md
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
    # Forgejo stores attachments on object storage that intermittently drops a PUT
    # (API 500): retry with growing pauses, removing a partial asset before each try.
    ok=""
    for attempt in 1 2 3 4 5 6; do
        if curl -fsS "${auth[@]}" -F "attachment=@${f}" \
            "$api/releases/$id/assets?name=${name}" >/dev/null; then
            ok=1
            break
        fi
        echo "upload of $name failed (attempt $attempt), retrying in $((attempt * 10)) s" >&2
        sleep $((attempt * 10))
        for a in $(curl -fsS "${auth[@]}" "$api/releases/$id/assets" |
            python3 -c 'import json,sys; print(" ".join(str(a["id"]) for a in json.load(sys.stdin) if a["name"] == sys.argv[1]))' "$name"); do
            curl -fsS -X DELETE "${auth[@]}" "$api/releases/$id/assets/$a" || true
        done
    done
    [ -n "$ok" ] || { echo "could not attach $name after 6 attempts" >&2; exit 1; }
    echo "attached $name to release $tag ($id)"
done
