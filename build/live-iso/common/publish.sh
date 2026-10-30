#!/usr/bin/env bash
# publish.sh <mode> <project> <iso> <evidence.tgz> <status-json>      (ARCH env: x86_64 (default) | aarch64 | riscv64)
#   mode=daily   -> rolling PRE-release tag `daily` (stable asset names, replaced in place)
#   mode=stable  -> attach to the release of the pushed v* tag (creates it if missing)
#   mode=status  -> only update the status marker release body (no assets): `daily-status` (x86_64) or `daily-status-<arch>`
# Env: FORGEJO_TOKEN, GITHUB_SERVER_URL, GITHUB_REPOSITORY, GITHUB_REF_NAME (tag, stable), GITHUB_SHA.
# Existing releases are edited, never deleted/recreated, so watchers are not re-notified.
set -euo pipefail
mode=$1; project=$2; iso=${3:-}; evidence=${4:-}; statusfile=${5:-}
: "${FORGEJO_TOKEN:?}" "${GITHUB_SERVER_URL:?}" "${GITHUB_REPOSITORY:?}"
api="$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY"
auth=(-H "Authorization: token $FORGEJO_TOKEN")
sha=${GITHUB_SHA:-$(git rev-parse HEAD)}
ARCH=${ARCH:-x86_64}
VARIANT=${VARIANT:-}
. "$(dirname "$0")/image-name.sh"      # IMG=<DE|server>-<init>, IMG_ID=<IMG>-<arch>: asset names + status marker (scheme: common/image-name.sh)
VPFX="$IMG-"
DAILY_TAG=${DAILY_TAG:-daily}      # tests publish to a throw-away tag

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
  marker=${DAILY_TAG}-status-$IMG_ID      # e.g. daily-status-server-openrc-x86_64, daily-status-gnome-light-openrc-x86_64 (old: daily-status, daily-status-<arch>, daily-status-desktop-x86_64)
  id=$(ensure_release "$marker" "$project daily build status $ARCH (machine-readable)" true "pending")
  patch_body "$id" "$(cat "$statusfile")" ;;
daily)
  size=$(du -h "$iso" | cut -f1); sum=$(cut -d' ' -f1 "$iso.sha256")
  name=$project-${VPFX}daily-$ARCH.iso
  id=$(ensure_release "$DAILY_TAG" "$project daily ISO (rolling, pre-release)" true "pending")
  [ -n "$id" ] || { echo "cannot create release"; exit 1; }
  put_asset "$id" "$iso" "$name"
  printf '%s  %s\n' "$sum" "$name" > /tmp/daily-${VPFX}$ARCH.sha256
  put_asset "$id" /tmp/daily-${VPFX}$ARCH.sha256 "$name.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-${VPFX}daily-$ARCH-test-evidence.tgz"
  # asset names of the older schemes (<p>-daily-<arch>.iso, <p>-desktop-daily-<arch>.iso, ...): removed once the same image is published under
  # its new name, so a stale file never looks current (the product-domain redirects keep the old URLs working)
  if true; then
    for old in "$project-daily-test-evidence.tgz" "$project-${IMG_OLD_VPFX}daily-$ARCH.iso" "$project-${IMG_OLD_VPFX}daily-$ARCH.iso.sha256" "$project-${IMG_OLD_VPFX}daily-$ARCH-test-evidence.tgz"; do
      for a in $(curl -fsS "${auth[@]}" "$api/releases/$id/assets" | python3 -c 'import json,sys;print(" ".join(str(a["id"]) for a in json.load(sys.stdin) if a["name"]==sys.argv[1]))' "$old"); do curl -fsS -X DELETE "${auth[@]}" "$api/releases/$id/assets/$a" || true; done
    done
  fi
  # release text: one line per architecture that has an ISO attached (each arch job rewrites it, assets are the truth)
  body=$(curl -fsS "${auth[@]}" "$api/releases/$id/assets" | ARCH=$ARCH SUMMARY="$(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d.get("summary",""))' "$statusfile" 2>/dev/null)" SHA=$sha PROJECT=$project python3 -c '
import json, os, re, sys, time
a = json.load(sys.stdin); p = os.environ["PROJECT"]
DE = {"server": "Server", "gnome-light": "GNOME (light)", "gnome": "GNOME", "xfce": "XFCE", "kde": "KDE"}
rows = []
for x in sorted(a, key=lambda x: x["name"]):
    n = x["name"]
    m = re.match(re.escape(p) + r"-((?:[a-z0-9]+-)*?[a-z0-9]+)-(openrc|systemd)-daily-(.+)\.iso$", n)
    if m:
        de = DE.get(m.group(1), m.group(1))
        rows.append("- **%s %s - %s**: `%s` (%d MiB), SHA256 in `%s.sha256`" % (de, m.group(2).replace("openrc", "OpenRC").replace("systemd", "systemd"), m.group(3), n, x["size"] >> 20, n))
print("Rolling **pre-release** build of the %s live/installer ISO from `main` (%s), updated %s UTC." % (p, os.environ["SHA"][:10], time.strftime("%F %T", time.gmtime())))
print("Each architecture is replaced in place by its own daily job, only after the QEMU boot + install + reboot + web-probe tests passed on that architecture.\n")
print("\n".join(rows)); print()
print("Last test result (%s): %s" % (os.environ["ARCH"], os.environ["SUMMARY"]))
print("\nWrite to USB: see docs/ISO-INSTALL.md (Secure Boot must be OFF). x86_64: UEFI or BIOS; aarch64 / riscv64: UEFI only.")
')
  patch_body "$id" "$body" true
  echo "DOWNLOAD $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/$DAILY_TAG/$name" ;;
stable)
  tag=${GITHUB_REF_NAME:?}
  id=$(ensure_release "$tag" "$tag" false "Release $tag")
  [ -n "$id" ] || { echo "no release for $tag"; exit 1; }
  n=$project-$IMG-$tag-$ARCH.iso      # <product>-<DE|server>-<init>-<tag>-<arch>.iso
  put_asset "$id" "$iso" "$n"
  printf '%s  %s\n' "$(cut -d' ' -f1 "$iso.sha256")" "$n" > /tmp/stable-$IMG-$ARCH.sha256
  put_asset "$id" /tmp/stable-$IMG-$ARCH.sha256 "$n.sha256"
  [ -z "$evidence" ] || put_asset "$id" "$evidence" "$project-$IMG-$tag-$ARCH-iso-test-evidence.tgz"
  echo "DOWNLOAD $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/$tag/$n" ;;
*) echo "bad mode"; exit 2 ;;
esac
