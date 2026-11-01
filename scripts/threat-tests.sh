#!/usr/bin/env bash
# Runs every test that crates/app-core/tests/fixtures/threat-tests.json names
# for a row of sync-v2.md §30. The test sync_threats checks the map itself.
set -euo pipefail
cd "$(dirname "$0")/.."

map=crates/app-core/tests/fixtures/threat-tests.json
for file in $(jq -r '.[].tests[] | split("::")[0]' "$map" | sort -u); do
  mapfile -t tests < <(jq -r --arg file "$file" \
    '.[].tests[] | split("::") | select(.[0] == $file) | .[1]' "$map" | sort -u)
  echo "$file: ${#tests[@]} tests"
  cargo test --locked -p app-core --test "$file" -- --exact "${tests[@]}"
done
