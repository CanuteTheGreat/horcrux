#!/usr/bin/env bash
# publish.sh <mode> <project> <iso> <evidence.tgz> <status-json>
#   mode=daily   -> rolling PRE-release tag `daily` (stable asset names, replaced in place)
#   mode=stable  -> attach to the release of the pushed v* tag (creates it if missing)
#   mode=status  -> only update the `daily-status` marker release body (no assets)
# Env: FORGEJO_TOKEN, GITHUB_SERVER_URL, GITHUB_REPOSITORY, GITHUB_REF_NAME (tag, stable), GITHUB_SHA.
# Existing releases are edited, never deleted/recreated, so watchers are not re-notified.
set -euo pipefail
mode=$1; project=$2; iso=${3:-}; evidence=${4:-}; statusfile=${5:-}
: "${FORGEJO_TOKEN:?}" "${GITHUB_SERVER_URL:?}" "${GITHUB_REPOSITORY:?}"
api="$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY"
auth=(-H "Authorization: token $FORGEJO_TOKEN")
sha=${GITHUB_SHA:-$(git rev-parse HEAD)}

rel_id() { curl -fsS "${auth[@]}" "$api/releases/tags/$1" 2>/dev/null | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])' 2>/dev/null || true; }
ensure_release() { # tag name prerelease body
  local id; id=$(rel_id "$1")
  if [ -z "$id" ]; then
    local b; b=$(python3 -c 'import json,sys;print(json.dumps({"tag_name":sys.argv[1],"target_commitish":sys.argv[5],"name":sys.argv[2],"body":sys.argv[4],"prerelease":sys.argv[3]=="true"}))' "$1" "$2" "$3" "$4" "$sha")
    curl -fsS "${auth[@]}" -H 'Content-Type: application/json' -d "$b" "$api/releases" >/dev/null || true
    id=$(rel_id "$1")
  fi
  echo "$id"
}
patch_body() { # id body [prerelease]
  local b; b=$(python3 -c 'import json,sys;d={"body":sys.argv[1]}
if len(sys.argv)>2: d["prerelease"]=sys.argv[2]=="true"
print(json.dumps(d))' "$2" ${3:+"$3"})
  curl -fsS "${auth[@]}" -X PATCH -H 'Content-Type: application/json' -d "$b" "$api/releases/$1" >/dev/null
}
put_asset() { # id file [name]
  local id=$1 f=$2 name=${3:-$(basename "$2")} old a
  old=$(curl -fsS "${auth[@]}" "$api/releases/$id/assets" | python3 -c 'import json,sys;print(" ".join(str(a["id"]) for a in json.load(sys.stdin) if a["name"]==sys.argv[1]))' "$name")
  for a in $old; do curl -fsS -X DELETE "${auth[@]}" "$api/releases/$id/assets/$a"; done
  curl -fsS --retry 3 "${auth[@]}" -F "attachment=@$f" "$api/releases/$id/assets?name=$name" >/dev/null
  echo "attached $name to release $id"
}

case "$mode" in
status)
  id=$(ensure_release daily-status "$project daily build status (machine-readable)" true "pending")
  patch_body "$id" "$(cat "$statusfile")" ;;
daily)
  size=$(du -h "$iso" | cut -f1); sum=$(cut -d' ' -f1 "$iso.sha256")
  body="Rolling **pre-release** build of the $project live/installer ISO from \`main\` ($sha), $(date -u +%F\ %T) UTC.
Replaced in place by the daily workflow after the QEMU boot + unattended install + reboot + web-probe test passed.

- ISO: \`$project-daily-x86_64.iso\` ($size)
- SHA256: \`$sum\`
- Write to USB: see docs/ISO-INSTALL.md (Secure Boot must be OFF)

$(cat "$statusfile" 2>/dev/null | python3 -c 'import json,sys;d=json.load(sys.stdin);print("Test result: "+d.get("summary",""))' 2>/dev/null)"
  id=$(ensure_release daily "$project daily ISO (rolling, pre-release)" true "$body")
  [ -n "$id" ] || { echo "cannot create release"; exit 1; }
  put_asset "$id" "$iso" "$project-daily-x86_64.iso"
  printf '%s  %s\n' "$sum" "$project-daily-x86_64.iso" > /tmp/daily.sha256
  put_asset "$id" /tmp/daily.sha256 "$project-daily-x86_64.iso.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-daily-test-evidence.tgz"
  patch_body "$id" "$body" true
  echo "DOWNLOAD $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/daily/$project-daily-x86_64.iso" ;;
stable)
  tag=${GITHUB_REF_NAME:?}
  id=$(ensure_release "$tag" "$tag" false "Release $tag")
  [ -n "$id" ] || { echo "no release for $tag"; exit 1; }
  n=$project-$tag-x86_64.iso
  put_asset "$id" "$iso" "$n"
  printf '%s  %s\n' "$(cut -d' ' -f1 "$iso.sha256")" "$n" > /tmp/stable.sha256
  put_asset "$id" /tmp/stable.sha256 "$n.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-$tag-iso-test-evidence.tgz"
  echo "DOWNLOAD $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/$tag/$n" ;;
*) echo "bad mode"; exit 2 ;;
esac
