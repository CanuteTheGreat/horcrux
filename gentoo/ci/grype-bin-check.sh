#!/bin/bash
# Real-Portage + real-scan check of app-containers/grype-bin (the Security > Vulnerabilities scanner), inside a stage3 container
# (never on the control host; run on an AWS rw-man host):
#   docker run --rm -v <checkout>:/src:ro --memory=4g gentoo/stage3:latest bash /src/gentoo/ci/grype-bin-check.sh
# 1) the overlay ebuild + Manifest resolve and install (digests verified by Portage), `grype version` runs, the package owns /usr/bin/grype;
# 2) the OFFLINE database recipe from docs/SECURITY-SUITE.md works: download the database archive, `grype db import` it, then scan with
#    GRYPE_DB_AUTO_UPDATE=false (no database download during the scan);
# 3) a known-vulnerable image (alpine:3.10) is scanned and has findings.
set -u
FAIL=0
emerge-webrsync -q >/dev/null 2>&1 || emerge --sync -q >/dev/null 2>&1
mkdir -p /var/db/repos/horcrux/metadata /var/db/repos/horcrux/profiles /etc/portage/repos.conf
cp -a /src/gentoo/app-containers /var/db/repos/horcrux/
printf 'masters = gentoo\nthin-manifests = true\n' > /var/db/repos/horcrux/metadata/layout.conf
echo horcrux > /var/db/repos/horcrux/profiles/repo_name
printf '[horcrux]\nlocation = /var/db/repos/horcrux\nmasters = gentoo\n' > /etc/portage/repos.conf/horcrux.conf

echo "=== 1. emerge"
emerge --oneshot --verbose app-containers/grype-bin >/tmp/emerge.log 2>&1 || { tail -30 /tmp/emerge.log; echo "FAIL: emerge grype-bin"; exit 1; }
qlist -Iv app-containers/grype-bin || FAIL=1
qfile /usr/bin/grype | grep -q grype-bin || { echo "FAIL: /usr/bin/grype not owned by grype-bin"; FAIL=1; }
grype version | tee /tmp/version.txt
grep -q '^Version:' /tmp/version.txt || { echo "FAIL: grype version"; FAIL=1; }

echo "=== 2. offline database recipe"
export GRYPE_CHECK_FOR_APP_UPDATE=false
listing=$(mktemp)
curl -fsSL https://grype.anchore.io/databases/v6/latest.json -o "$listing" || { echo "FAIL: database listing"; exit 1; }
# latest.json: {"status":..,"schemaVersion":..,"built":..,"path":"vulnerability-db_v6.x.y_<date>.tar.zst","checksum":"sha256:.."}
path=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["path"])' "$listing")
sum=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["checksum"].split(":",1)[-1])' "$listing")
mkdir -p /tmp/dl && curl -fsSL "https://grype.anchore.io/databases/v6/$path" -o "/tmp/dl/db.tar.zst" || { echo "FAIL: database download"; exit 1; }
[ "$(sha256sum /tmp/dl/db.tar.zst | cut -d' ' -f1)" = "$sum" ] || { echo "FAIL: database checksum mismatch"; FAIL=1; }
export GRYPE_DB_CACHE_DIR=/tmp/scanner-db
grype db import /tmp/dl/db.tar.zst || { echo "FAIL: grype db import"; FAIL=1; }
grype db status || FAIL=1

echo "=== 3. scan alpine:3.10 with the imported database only"
export GRYPE_DB_AUTO_UPDATE=false GRYPE_DB_VALIDATE_AGE=false
grype registry:alpine:3.10 -o json -q > /tmp/scan.json || { echo "FAIL: scan"; FAIL=1; }
python3 - <<'EOF' || FAIL=1
import json,collections
d=json.load(open('/tmp/scan.json'))
m=d.get('matches',[])
c=collections.Counter(x['vulnerability']['severity'] for x in m)
print('findings:',len(m),dict(c))
raise SystemExit(0 if len(m)>0 else 1)
EOF
[ $FAIL = 0 ] && echo "ALL OK" || echo "SOME FAILED"
exit $FAIL
