#!/usr/bin/env bash
# Tell IndexNow (Bing, Yandex, Seznam, Naver) which URLs of this site exist/changed: submits every <loc> of the
# live sitemap. Needs the key file https://HOST/KEY.txt to be served (it is a public file by design).
# usage: ci/indexnow.sh HOST KEY [SITEMAP_URL]   (always exits 0: indexing pings must never fail a deploy)
set -u
HOST="$1"; KEY="$2"; SITEMAP="${3:-https://$HOST/sitemap.xml}"
curl -fsS --max-time 30 "$SITEMAP" -o /tmp/indexnow-sitemap.xml || { echo "indexnow: cannot fetch $SITEMAP (skipped)"; exit 0; }
URLS=$(grep -o '<loc>[^<]*</loc>' /tmp/indexnow-sitemap.xml | sed -e 's#<loc>##' -e 's#</loc>##' | grep "^https://$HOST/" | grep -v '#' | sed 's/.*/"&"/' | paste -sd, -)
[ -n "$URLS" ] || { echo "indexnow: no URLs for $HOST (skipped)"; exit 0; }
BODY="{\"host\":\"$HOST\",\"key\":\"$KEY\",\"keyLocation\":\"https://$HOST/$KEY.txt\",\"urlList\":[$URLS]}"
CODE=$(curl -sS --max-time 30 -o /tmp/indexnow-resp.txt -w '%{http_code}' -X POST https://api.indexnow.org/IndexNow \
  -H 'Content-Type: application/json; charset=utf-8' -d "$BODY") || CODE=000
echo "indexnow $HOST: HTTP $CODE ($(echo "$URLS" | tr ',' '\n' | wc -l) urls)"; cat /tmp/indexnow-resp.txt 2>/dev/null
rm -f /tmp/indexnow-sitemap.xml /tmp/indexnow-resp.txt
exit 0
