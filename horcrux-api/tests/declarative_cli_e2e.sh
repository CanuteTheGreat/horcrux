#!/bin/bash
# `horcrux` CLI against a real horcrux-api, for the declarative commands: desired set/get/validate/apply, drift (exit codes),
# history list/show/rollback, apply --rollback, gitops config/status/sync/approve/reject. The server runs on a free port with
# throwaway state; everything goes through the real CLI binary (login included).
# Usage: horcrux-api/tests/declarative_cli_e2e.sh <horcrux-api binary> <horcrux CLI binary>
set -euo pipefail
API=$(readlink -f "${1:?usage: $0 <horcrux-api> <horcrux>}")
CLI=$(readlink -f "${2:?usage: $0 <horcrux-api> <horcrux>}")
D=$(mktemp -d)
PID=""
cleanup() { [ -n "$PID" ] && kill "$PID" 2>/dev/null || true; rm -rf "$D"; }
trap cleanup EXIT
PORT=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')
mkdir -p "$D/data" "$D/cni/bin" "$D/cni/conf.d" "$D/home"
HORCRUX_HOST=127.0.0.1 HORCRUX_PORT=$PORT HORCRUX_DATA_DIR="$D/data" HORCRUX_DATABASE_URL="sqlite://$D/data/horcrux.db" \
HORCRUX_SSL_DIR="$D/ssl" HORCRUX_LOG_DIR="$D/log" HORCRUX_AUDIT_LOG="$D/log/audit.log" HORCRUX_QEMU_RUN_DIR="$D/run" \
HORCRUX_CNI_BIN_DIR="$D/cni/bin" HORCRUX_CNI_CONF_DIR="$D/cni/conf.d" ADMIN_PASSWORD=admin \
RATE_LIMIT_MAX_REQUESTS=100000 AUTH_RATE_LIMIT_MAX_REQUESTS=100000 RUST_LOG=info nohup "$API" > "$D/api.log" 2>&1 &
PID=$!
for i in $(seq 1 60); do curl -skf "https://127.0.0.1:$PORT/api/health" >/dev/null && break; sleep 1; done
curl -skf "https://127.0.0.1:$PORT/api/health" >/dev/null || { echo "API did not start"; tail -30 "$D/api.log"; exit 1; }

export HOME="$D/home" HORCRUX_INSECURE=1
cli() { "$CLI" --server "https://127.0.0.1:$PORT" "$@"; }
fail() { echo "FAIL: $*"; tail -20 "$D/api.log"; exit 1; }
expect_exit() { # expected-code command...
  local want=$1; shift
  set +e; "$@" > "$D/out.txt" 2>&1; local got=$?; set -e
  [ "$got" = "$want" ] || { cat "$D/out.txt"; fail "'$*' exited $got, wanted $want"; }
}
ok() { echo "ok - $*"; }

cli auth login -u admin -p admin > /dev/null || fail "login"
ok "login"

cat > "$D/spec1.yaml" <<EOF
version: 1
vms:
  - {name: cli-a, memory: 64, cpus: 1, power: stopped}
EOF
cat > "$D/spec2.yaml" <<EOF
version: 1
vms:
  - {name: cli-a, memory: 96, cpus: 1, power: stopped}
  - {name: cli-b, memory: 64, cpus: 1, power: stopped}
EOF

# nothing stored: a clear error, not a crash
expect_exit 1 cli desired get
grep -q "no desired spec" "$D/out.txt" || fail "desired get without a spec: $(cat "$D/out.txt")"
echo "version: 9" > "$D/bad.yaml"
expect_exit 1 cli desired validate -f "$D/bad.yaml"
grep -q "not supported" "$D/out.txt" || fail "validate of a bad spec: $(cat "$D/out.txt")"
expect_exit 0 cli desired validate -f "$D/spec1.yaml"
grep -q "cli-a" "$D/out.txt" || fail "validate plan: $(cat "$D/out.txt")"
ok "desired validate (bad spec refused, good spec planned)"

expect_exit 0 cli desired set -f "$D/spec1.yaml"
expect_exit 0 cli desired get
diff <(cat "$D/spec1.yaml") <(cat "$D/out.txt") > /dev/null || fail "desired get differs from what was set"
expect_exit 2 cli drift --refresh
grep -q "cli-a" "$D/out.txt" && grep -q "missing" "$D/out.txt" || fail "drift output: $(cat "$D/out.txt")"
ok "desired set/get, drift exits 2 and names the missing VM"

expect_exit 0 cli desired apply --dry-run
expect_exit 0 cli desired apply
expect_exit 0 cli drift --refresh
grep -q "in sync" "$D/out.txt" || fail "drift after apply: $(cat "$D/out.txt")"
ok "desired apply, drift exits 0"

expect_exit 0 cli history list
grep -q "cli " "$D/out.txt" || grep -q "cli" "$D/out.txt" || fail "history list: $(cat "$D/out.txt")"
H1=$(awk 'NR==1{print $1}' "$D/out.txt")
expect_exit 0 cli history show "$H1"
grep -q "applied" "$D/out.txt" || fail "history show: $(cat "$D/out.txt")"
expect_exit 0 cli history show "$H1" --spec
diff "$D/spec1.yaml" "$D/out.txt" > /dev/null || fail "history show --spec differs"
ok "history list/show"

# hand drift through the API, then the CLI sees and fixes it
TOKEN=$(python3 - "$HOME/.config/horcrux/cli.toml" <<'EOF'
import re, sys
print(re.search(r'token\s*=\s*"([^"]+)"', open(sys.argv[1]).read()).group(1))
EOF
)
curl -skf -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{"memory": 200}' "https://127.0.0.1:$PORT/api/vms/cli-a/config" > /dev/null
expect_exit 2 cli drift --refresh
grep -q "memory" "$D/out.txt" && grep -q "expected 64" "$D/out.txt" && grep -q "actual 200" "$D/out.txt" || fail "drift detail: $(cat "$D/out.txt")"
expect_exit 0 cli desired apply
expect_exit 0 cli drift --refresh
ok "hand change shows as drift (expected 64, actual 200) and desired apply corrects it"

# apply a bigger spec, then roll it back
expect_exit 0 cli apply -f "$D/spec2.yaml"
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-b || fail "cli-b not created"
expect_exit 0 cli apply --rollback --dry-run
grep -q "cli-b" "$D/out.txt" || fail "rollback preview: $(cat "$D/out.txt")"
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-b || fail "the preview removed cli-b"
expect_exit 0 cli apply --rollback
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-b && fail "cli-b survived the rollback"
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-a || fail "rollback removed cli-a"
expect_exit 0 cli history list
grep -q rollback "$D/out.txt" || fail "rollback not in the history: $(cat "$D/out.txt")"
ok "apply, apply --rollback --dry-run, apply --rollback"

# GitOps through the CLI against a real repository
G="$D/repo"; mkdir -p "$G"
git_() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -c user.name=CI -c user.email=ci@example.test -C "$G" "$@"; }
git_ init -q -b main
cp "$D/spec2.yaml" "$G/horcrux.yaml"; git_ add -A; git_ commit -q -m "bigger spec"
cat > "$D/git.yaml" <<EOF
url: file://$G
mode: approve
interval_secs: 3600
EOF
expect_exit 0 cli gitops status
grep -q "not configured" "$D/out.txt" || fail "status before config: $(cat "$D/out.txt")"
cat > "$D/gitbad.yaml" <<EOF
url: file://$G
credentials: {secret: hunter2}
EOF
expect_exit 1 cli gitops config -f "$D/gitbad.yaml"
grep -q "references" "$D/out.txt" || fail "plaintext secret accepted: $(cat "$D/out.txt")"
expect_exit 0 cli gitops config -f "$D/git.yaml"
expect_exit 0 cli gitops sync
HASH=$(grep -o 'gitops approve [0-9a-f]*' "$D/out.txt" | awk '{print $3}')
[ -n "$HASH" ] || fail "sync didn't queue an approval: $(cat "$D/out.txt")"
expect_exit 0 cli gitops status
grep -q "pending" "$D/out.txt" && grep -q "bigger spec" "$D/out.txt" || fail "status pending: $(cat "$D/out.txt")"
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-b && fail "applied before approval"
expect_exit 0 cli gitops approve "$HASH"
curl -skf -H "Authorization: Bearer $TOKEN" "https://127.0.0.1:$PORT/api/vms" | grep -q cli-b || fail "approve didn't apply"
# a second commit, rejected
sed -i 's/memory: 96/memory: 128/' "$G/horcrux.yaml"; git_ commit -q -am "more memory"
expect_exit 0 cli gitops sync
HASH2=$(grep -o 'gitops approve [0-9a-f]*' "$D/out.txt" | awk '{print $3}')
[ -n "$HASH2" ] || fail "second commit not queued"
expect_exit 0 cli gitops reject "$HASH2" --reason "not now"
expect_exit 0 cli gitops status
grep -q "rejected" "$D/out.txt" || fail "status after reject: $(cat "$D/out.txt")"
expect_exit 1 cli gitops approve "$HASH2"
expect_exit 0 cli gitops disable
expect_exit 0 cli desired clear
ok "gitops config (plaintext refused), sync, status, approve, reject, disable"
echo "declarative CLI e2e: all ok"
