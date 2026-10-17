#!/bin/bash
# Authentication realms + ACME against real servers, each in its own
# container on a private network:
#   - OpenLDAP (Debian slapd) with ou=people / uid=alice
#   - Dex (OpenID Connect provider) with a static client and user
#   - Pebble + pebble-challtestsrv (ACME CA, validates HTTP-01 for real by
#     resolving the test domain to the runner container)
#   - a runner container with the built horcrux-api (HTTPS, redirect port
#     5002 for HTTP-01, a PAM service and a host account) where the
#     auth_realms_e2e test binary runs.
# Usage: auth_realms_e2e.sh <horcrux-api binary> <auth_realms_e2e test binary> [image]
set -euo pipefail
API=${1:?usage: $0 <horcrux-api> <test binary> [image]}
TESTBIN=${2:?usage: $0 <horcrux-api> <test binary> [image]}
IMAGE=${3:-catthehacker/ubuntu:act-latest}
ID="hxr$$"
NET="$ID-net"
TMP=$(mktemp -d)

cleanup() {
  rc=$?
  if [ $rc -ne 0 ]; then
    echo "::group::horcrux-api log"; docker exec "$ID-run" tail -150 /tmp/hx/api.log 2>/dev/null || true; echo "::endgroup::"
    echo "::group::dex log"; docker logs --tail 80 "$ID-dex" 2>&1 || true; echo "::endgroup::"
    echo "::group::pebble log"; docker logs --tail 80 "$ID-pebble" 2>&1 || true; echo "::endgroup::"
  fi
  for c in run ldap dex pebble chall; do docker rm -f "$ID-$c" >/dev/null 2>&1 || true; done
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$TMP"
  exit $rc
}
trap cleanup EXIT

docker network create "$NET" >/dev/null

echo "== runner"
docker run -d --name "$ID-run" --network "$NET" --network-alias horcrux.test --memory 2g "$IMAGE" sleep 3600 >/dev/null
RUN_IP=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$ID-run")
echo "runner at $RUN_IP"

echo "== OpenLDAP"
docker run -d --name "$ID-ldap" --network "$NET" --network-alias ldap debian:bookworm-slim sleep 3600 >/dev/null
docker exec "$ID-ldap" bash -c '
set -e
export DEBIAN_FRONTEND=noninteractive
cat <<EOF | debconf-set-selections
slapd slapd/domain string horcrux.test
slapd shared/organization string Horcrux
slapd slapd/password1 password ldap-admin-pass
slapd slapd/password2 password ldap-admin-pass
slapd slapd/no_configuration boolean false
EOF
apt-get update -qq >/dev/null
apt-get install -y -qq slapd ldap-utils >/dev/null
(slapd -h "ldap:///" -u openldap -g openldap || true)
for i in $(seq 1 30); do ldapsearch -x -H ldap://localhost -b dc=horcrux,dc=test -s base >/dev/null 2>&1 && break; sleep 1; done
ldapadd -x -H ldap://localhost -D cn=admin,dc=horcrux,dc=test -w ldap-admin-pass >/dev/null <<EOF
dn: ou=people,dc=horcrux,dc=test
objectClass: organizationalUnit
ou: people

dn: uid=alice,ou=people,dc=horcrux,dc=test
objectClass: inetOrgPerson
uid: alice
cn: Alice Example
sn: Example
mail: alice@horcrux.test
userPassword: alice-ldap-pass
EOF
ldapwhoami -x -H ldap://localhost -D uid=alice,ou=people,dc=horcrux,dc=test -w alice-ldap-pass
'

echo "== Dex"
cat > "$TMP/dex.yaml" <<'EOF'
issuer: http://dex:5556/dex
storage:
  type: memory
web:
  http: 0.0.0.0:5556
oauth2:
  skipApprovalScreen: true
enablePasswordDB: true
staticClients:
  - id: horcrux
    name: Horcrux
    secret: horcrux-ci-secret
    redirectURIs:
      - https://localhost:8006/api/auth/oidc/callback
staticPasswords:
  - email: carol@horcrux.test
    # bcrypt of "password" (Dex's own example hash)
    hash: "$2a$10$2b2cU8CPhOTaGrs1HRQuAueS7JTT5ZHsHSzYiFPm1leZck7Mc8T4W"
    username: carol
    userID: 5f0a3c2e-0c1b-4a54-9d4b-6d2b7d1e3a11
EOF
docker create --name "$ID-dex" --network "$NET" --network-alias dex ghcr.io/dexidp/dex:v2.41.1 dex serve /etc/dex/horcrux.yaml >/dev/null
docker cp "$TMP/dex.yaml" "$ID-dex":/etc/dex/horcrux.yaml
docker start "$ID-dex" >/dev/null

echo "== Pebble"
docker run -d --name "$ID-chall" --network "$NET" --network-alias challtestsrv \
  ghcr.io/letsencrypt/pebble-challtestsrv:latest -defaultIPv6 "" -defaultIPv4 "$RUN_IP" >/dev/null
docker run -d --name "$ID-pebble" --network "$NET" --network-alias pebble -e PEBBLE_VA_NOSLEEP=1 \
  ghcr.io/letsencrypt/pebble:latest -config test/config/pebble-config.json -dnsserver challtestsrv:8053 >/dev/null
sleep 2
docker cp "$ID-pebble":/test/certs/pebble.minica.pem "$TMP/pebble-ca.pem"

echo "== horcrux-api in the runner"
docker cp "$API" "$ID-run":/usr/local/bin/horcrux-api
docker cp "$TESTBIN" "$ID-run":/usr/local/bin/auth-realms-e2e
docker cp "$TMP/pebble-ca.pem" "$ID-run":/etc/pebble-ca.pem
docker exec "$ID-run" bash -c '
set -e
(ldconfig -p | grep -q libvirt.so.0 && command -v openssl >/dev/null) || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libvirt0 openssl >/dev/null)
id pamtester >/dev/null 2>&1 || useradd -m pamtester
echo "pamtester:pam-host-pass-1" | chpasswd
printf "auth required pam_unix.so\naccount required pam_unix.so\n" > /etc/pam.d/horcrux
mkdir -p /tmp/hx/cni/bin /tmp/hx/cni/conf
cd /tmp/hx
HORCRUX_HOST=0.0.0.0 HORCRUX_PORT=8006 HORCRUX_HTTP_REDIRECT_PORT=5002 \
HORCRUX_DATA_DIR=/tmp/hx HORCRUX_SSL_DIR=/tmp/hx/ssl HORCRUX_DATABASE_URL=sqlite:///tmp/hx/horcrux.db \
HORCRUX_CNI_BIN_DIR=/tmp/hx/cni/bin HORCRUX_CNI_CONF_DIR=/tmp/hx/cni/conf \
ADMIN_PASSWORD=admin RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info \
  nohup horcrux-api > /tmp/hx/api.log 2>&1 &
for i in $(seq 1 60); do curl -skf https://localhost:8006/api/health >/dev/null && exit 0; sleep 1; done
echo "horcrux-api did not come up"; exit 1
'
for i in $(seq 1 30); do docker exec "$ID-run" curl -sf http://dex:5556/dex/.well-known/openid-configuration >/dev/null && break; sleep 1; done

echo "== realm + ACME tests"
docker exec \
  -e HORCRUX_TEST_API=https://localhost:8006 \
  -e LDAP_HOST=ldap -e LDAP_ADMIN_PASSWORD=ldap-admin-pass \
  -e DEX_ISSUER=http://dex:5556/dex \
  -e PAM_USER=pamtester -e PAM_PASSWORD=pam-host-pass-1 \
  -e ACME_DIR=https://pebble:14000/dir -e ACME_CA=/etc/pebble-ca.pem -e ACME_DOMAIN=horcrux.test \
  "$ID-run" auth-realms-e2e --ignored --test-threads=1 --nocapture

echo "== the HTTPS listener presents the ACME certificate"
issuer=$(docker exec "$ID-run" bash -c 'echo | openssl s_client -connect localhost:8006 -servername horcrux.test 2>/dev/null | openssl x509 -noout -issuer')
echo "$issuer"
echo "$issuer" | grep -qi pebble
echo "realms e2e: LDAP, OpenID Connect (Dex), PAM and ACME (Pebble) all passed"
