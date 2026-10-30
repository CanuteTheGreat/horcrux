#!/bin/bash
# publish-surface-nextcloud.sh ISO EXPIRY(YYYY-MM-DD)
# Private delivery of the Surface image (owner rule: a public share of a THROW-AWAY non-owner Nextcloud user, with an expiry; never a public site).
# Creates the user `surface-share` (quota 10 GB), uploads the ISO over WebDAV, creates a read-only public link with the expiry, prints the
# direct download URL, then verifies it (HTTP status + sha256 of the downloaded bytes). Delete the user afterwards: occ user:delete surface-share
set -euo pipefail
ISO=$1; EXP=$2
NC=https://nextcloud.canutethegreat.com
PW=$(head -c 24 /dev/urandom | base64 | tr -d '/+=' | head -c 24)
cat > /tmp/nc-user.sh <<EOF
set -e
cd /var/www/html
export OC_PASS='$PW'
sudo -u www-data -E php occ user:delete surface-share >/dev/null 2>&1 || true
sudo -u www-data -E php occ user:add --password-from-env --display-name "Surface image share (temporary)" surface-share
sudo -u www-data php occ user:setting surface-share files quota "10 GB"
EOF
scp -q /tmp/nc-user.sh ollivanders:/tmp/nc-user.sh
ssh ollivanders 'sudo lxc file push -q /tmp/nc-user.sh nextcloud/tmp/nc-user.sh && sudo lxc exec nextcloud -- bash /tmp/nc-user.sh; sudo lxc exec nextcloud -- rm -f /tmp/nc-user.sh; rm -f /tmp/nc-user.sh'
rm -f /tmp/nc-user.sh
NAME=$(basename "$ISO")
curl -sS --fail -u "surface-share:$PW" -T "$ISO" "$NC/remote.php/dav/files/surface-share/$NAME" -o /dev/null -w "upload HTTP %{http_code}\n"
R=$(curl -sS -u "surface-share:$PW" -H 'OCS-APIRequest: true' -H 'Accept: application/json' \
    -d "path=/$NAME" -d shareType=3 -d permissions=1 -d "expireDate=$EXP" "$NC/ocs/v2.php/apps/files_sharing/api/v1/shares")
TOKEN=$(echo "$R" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['ocs']['data']['token'])")
URL="$NC/index.php/s/$TOKEN/download"
echo "LINK $URL"
curl -sS -L -o /tmp/nc-verify.bin -w "verify HTTP %{http_code} size %{size_download}\n" "$URL"
echo "sha256 $(sha256sum /tmp/nc-verify.bin | cut -d' ' -f1)"; rm -f /tmp/nc-verify.bin
