#!/usr/bin/env bash
# Purge the whole Cloudflare cache of ONE zone after a deploy.
# Needs CF_PURGE_TOKEN (zone-scoped, Cache Purge only) and CF_ZONE_ID (repo Actions secrets).
# Retries 429 (honours Retry-After) and 5xx / network errors; any other failure is final.
# Exits non-zero (loudly) when the purge did not succeed. Never prints the token.
set -u
: "${CF_PURGE_TOKEN:?CF_PURGE_TOKEN is not set (repo Actions secret)}"
: "${CF_ZONE_ID:?CF_ZONE_ID is not set (repo Actions secret)}"
url="https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/purge_cache"
out=$(mktemp); trap 'rm -f "$out"' EXIT
max=6
for attempt in $(seq 1 "$max"); do
  # token goes through a curl config on stdin, never argv
  code=$(printf 'header = "Authorization: Bearer %s"\n' "$CF_PURGE_TOKEN" | curl -sS -K - \
    -o "$out" -D "$out.h" -w '%{http_code}' --max-time 30 \
    -X POST -H 'Content-Type: application/json' --data '{"purge_everything":true}' "$url" 2>"$out.e") || code=000
  if [ "$code" = 200 ] && grep -q '"success": *true' "$out"; then
    echo "Cloudflare purge OK (zone ${CF_ZONE_ID}, attempt ${attempt})"; rm -f "$out.h" "$out.e"; exit 0
  fi
  wait=$((attempt * 5))
  case "$code" in
    429) ra=$(grep -i '^retry-after:' "$out.h" 2>/dev/null | tr -dc '0-9'); [ -n "$ra" ] && wait=$ra ;;
    5??|000) : ;;
    *) echo "::error::Cloudflare purge FAILED, HTTP $code (not retryable): $(head -c 400 "$out")" >&2; exit 1 ;;
  esac
  echo "purge attempt ${attempt}/${max}: HTTP ${code}; retrying in ${wait}s" >&2
  [ "$attempt" = "$max" ] || sleep "$wait"
done
echo "::error::Cloudflare purge FAILED after ${max} attempts - the deploy is live but stale pages may stay cached (up to 4 h). Purge by hand: bash ~/.hermes/scripts/cloudflare-purge.sh <domain>" >&2
exit 1
