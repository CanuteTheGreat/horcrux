#!/usr/bin/env bash
# publish-github.sh <mode> <project> <iso> <evidence.tgz> <status-json>      (ARCH env: x86_64 (default) | aarch64 | riscv64)
# Second publish step, run by ci-run.sh AFTER publish.sh (Forgejo): puts the same files on the public GitHub mirror
# (github.com/CanuteTheGreat/<project>) so the product site can redirect its stable download URLs there.
#   mode=daily  -> rolling PRE-release tag `daily` (stable asset names, replaced in place)
#   mode=stable -> release of the pushed tag (GITHUB_REF_NAME), a normal (non-pre) release
# Env: FORGEJO_TOKEN (optional, to trigger a mirror sync), GH_RELEASE_TOKEN (Forgejo repo secret; a GitHub token with `repo` scope), GITHUB_SHA, GITHUB_REF_NAME (stable),
#      GH_REPO (default CanuteTheGreat/<project>), DAILY_TAG (default daily; tests use a throw-away tag).
# Never fails the build: the caller treats a non-zero exit as "github publish failed" only in the job summary.
# GitHub allows 2 GiB per release asset; a bigger ISO is refused here with a clear message (split it first).
set -euo pipefail
mode=$1; project=$2; iso=${3:-}; evidence=${4:-}; statusfile=${5:-}
: "${GH_RELEASE_TOKEN:?}"
repo=${GH_REPO:-CanuteTheGreat/$project}
api=https://api.github.com/repos/$repo
upl=https://uploads.github.com/repos/$repo
H=(-H "Authorization: Bearer $GH_RELEASE_TOKEN" -H "Accept: application/vnd.github+json" -H "X-GitHub-Api-Version: 2022-11-28")
sha=${GITHUB_SHA:-$(git rev-parse HEAD)}
ARCH=${ARCH:-x86_64}
VARIANT=${VARIANT:-}; VPFX=${VARIANT:+$VARIANT-}
DAILY_TAG=${DAILY_TAG:-daily}
LIMIT=2147483648
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
pyj() { python3 -c "$@"; }

rel_json() { curl -fsS "${H[@]}" "$api/releases/tags/$1" 2>/dev/null || true; }
rel_id() { rel_json "$1" | pyj 'import json,sys
try: print(json.load(sys.stdin)["id"])
except Exception: pass' 2>/dev/null || true; }
# GitHub is a Forgejo PUSH MIRROR (git push --mirror): never create tags/branches there, the mirror would prune them.
# Releases are only attached to tags that already arrived from Forgejo; if the tag is not there yet, ask Forgejo to
# sync the mirror (best effort, needs FORGEJO_TOKEN) and wait for it.
tag_on_github() { curl -fsS "${H[@]}" "$api/git/ref/tags/$1" >/dev/null 2>&1; }
wait_for_tag() {
  tag_on_github "$1" && return 0
  if [ -n "${FORGEJO_TOKEN:-}" ] && [ -n "${GITHUB_SERVER_URL:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ]; then
    curl -fsS -X POST -H "Authorization: token $FORGEJO_TOKEN" "$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY/push_mirrors-sync" >/dev/null 2>&1 \
      && echo "asked Forgejo to sync the GitHub mirror" || echo "could not trigger a mirror sync (token lacks rights?); waiting for the scheduled/on-commit sync"
  fi
  local i; for i in $(seq 1 30); do sleep 10; tag_on_github "$1" && return 0; done
  return 1
}
ensure_release() { # tag name prerelease body
  local id; id=$(rel_id "$1")
  if [ -z "$id" ]; then
    wait_for_tag "$1" || { echo "tag $1 is not on the GitHub mirror yet; not creating it from here" >&2; return 0; }
    local b; b=$(pyj 'import json,sys;print(json.dumps({"tag_name":sys.argv[1],"name":sys.argv[2],"body":sys.argv[4],"prerelease":sys.argv[3]=="true","make_latest":"false"}))' "$1" "$2" "$3" "$4")
    curl -fsS "${H[@]}" -H 'Content-Type: application/json' -d "$b" "$api/releases" >/dev/null || true
    id=$(rel_id "$1")
  fi
  echo "$id"
}
patch_body() { # id body prerelease
  local b; b=$(pyj 'import json,sys;print(json.dumps({"body":sys.argv[1],"prerelease":sys.argv[2]=="true"}))' "$2" "$3")
  curl -fsS "${H[@]}" -X PATCH -H 'Content-Type: application/json' -d "$b" "$api/releases/$1" >/dev/null
}
assets_json() { curl -fsS "${H[@]}" "$api/releases/$1/assets?per_page=100"; }
put_asset() { # id file name   (delete the same-named asset, then upload)
  local id=$1 f=$2 name=$3 size a
  size=$(stat -c %s "$f")
  if [ "$size" -ge "$LIMIT" ]; then echo "::error::$name is $size bytes, over GitHub's 2 GiB per-asset limit; not uploaded"; return 1; fi
  for a in $(assets_json "$id" | pyj 'import json,sys;print(" ".join(str(a["id"]) for a in json.load(sys.stdin) if a["name"]==sys.argv[1]))' "$name"); do
    curl -fsS -X DELETE "${H[@]}" "$api/releases/assets/$a"
  done
  local ct=application/octet-stream; case $name in *.sha256) ct=text/plain;; *.tgz) ct=application/gzip;; esac
  curl -fsS --retry 3 --retry-delay 10 --max-time 3600 "${H[@]}" -H "Content-Type: $ct" -X POST -T "$f" "$upl/releases/$id/assets?name=$name" >/dev/null
  echo "github: attached $name ($((size>>20)) MiB) to release $id"
}

case "$mode" in
daily)
  [ -s "$iso" ] || { echo "no ISO at $iso"; exit 1; }
  sum=$(cut -d' ' -f1 "$iso.sha256"); name=$project-${VPFX}daily-$ARCH.iso
  id=$(ensure_release "$DAILY_TAG" "$project daily ISO (rolling, pre-release)" true "pending")
  [ -n "$id" ] || { echo "cannot create GitHub release $DAILY_TAG"; exit 1; }
  put_asset "$id" "$iso" "$name"
  printf '%s  %s\n' "$sum" "$name" > "$tmp/$name.sha256"
  put_asset "$id" "$tmp/$name.sha256" "$name.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-${VPFX}daily-$ARCH-test-evidence.tgz"
  body=$(assets_json "$id" | ARCH=$ARCH SUMMARY="$(pyj 'import json,sys;print(json.load(open(sys.argv[1])).get("summary",""))' "$statusfile" 2>/dev/null || true)" SHA=$sha PROJECT=$project python3 -c '
import json, os, re, sys, time
a = json.load(sys.stdin); p = os.environ["PROJECT"]
rows = []
for x in sorted(a, key=lambda x: x["name"]):
    n = x["name"]
    m = re.match(re.escape(p) + r"-((?:desktop-)?)daily-(.+)\.iso$", n)
    if m:
        rows.append("- **%s%s**: `%s` (%d MiB), SHA256 in `%s.sha256`" % (m.group(2), " (Desktop: GNOME, encrypted ZFS, Apple T2 ready)" if m.group(1) else "", n, x["size"] >> 20, n))
print("Rolling **pre-release** build of the %s live/installer ISO from `main` (%s), updated %s UTC." % (p, os.environ["SHA"][:10], time.strftime("%F %T", time.gmtime())))
print("Each architecture is replaced in place by its own daily job, only after the QEMU boot + install + reboot + web-probe tests passed on that architecture.\n")
print("\n".join(rows)); print()
print("Last test result (%s): %s" % (os.environ["ARCH"], os.environ["SUMMARY"]))
print("\nTest builds, not releases: use test hardware or a VM. Secure Boot must be OFF. Install guide: https://%s.errant.solutions/docs/iso-install/" % p)
')
  patch_body "$id" "$body" true
  echo "GITHUB DOWNLOAD https://github.com/$repo/releases/download/$DAILY_TAG/$name" ;;
stable)
  tag=${GITHUB_REF_NAME:?}
  [ -s "$iso" ] || { echo "no ISO at $iso"; exit 1; }
  id=$(ensure_release "$tag" "$tag" false "Release $tag. Install guide: https://$project.errant.solutions/docs/iso-install/")
  [ -n "$id" ] || { echo "no GitHub release for $tag"; exit 1; }
  n=$project-$tag-$ARCH.iso
  put_asset "$id" "$iso" "$n"
  printf '%s  %s\n' "$(cut -d' ' -f1 "$iso.sha256")" "$n" > "$tmp/$n.sha256"
  put_asset "$id" "$tmp/$n.sha256" "$n.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-$tag-$ARCH-iso-test-evidence.tgz"
  echo "GITHUB DOWNLOAD https://github.com/$repo/releases/download/$tag/$n" ;;
*) echo "bad mode"; exit 2 ;;
esac
