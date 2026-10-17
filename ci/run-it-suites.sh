#!/usr/bin/env bash
# Run horcrux-api's consolidated integration-test binary (tests/it) one
# suite at a time, in name order - the way the suites ran when each was its
# own test binary. Many of them talk to the same live server, so they must
# not run concurrently with each other (tests inside a suite still run in
# parallel as before).
#
# usage: ci/run-it-suites.sh [cargo test options, e.g. --all-features]
#
# Suites are selected by exact test names (libtest filters are substrings:
# "integration_tests::" would also match "oidc_integration_tests::...").
# Every suite runs even if an earlier one failed; the script fails if any did.
set -uo pipefail

list=$(cargo test "$@" -p horcrux-api --test it -- --list --format terse) || exit 1
suites=$(echo "$list" | awk -F': ' '$2 == "test" { split($1, p, "::"); print p[1] }' | sort -u)
[ -n "$suites" ] || { echo "no tests found in horcrux-api --test it" >&2; exit 1; }

failed=""
for s in $suites; do
    names=$(echo "$list" | awk -F': ' -v pre="$s::" '$2 == "test" && index($1, pre) == 1 { print $1 }')
    echo "::group::integration suite $s ($(echo "$names" | wc -l) tests)"
    # shellcheck disable=SC2086 # one argument per test name
    cargo test "$@" -p horcrux-api --test it -- --exact $names || failed="$failed $s"
    echo "::endgroup::"
done
if [ -n "$failed" ]; then
    echo "integration suites with failures:$failed" >&2
    exit 1
fi
echo "all integration suites passed: $(echo $suites)"
