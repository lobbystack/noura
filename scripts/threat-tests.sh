#!/usr/bin/env bash
# Runs every test that crates/app-core/tests/fixtures/threat-tests.json names
# for a row of sync-v2.md §30. The test sync_threats checks the map itself.
# Each name is `module::function` in app-core's `it` test binary.
set -euo pipefail
cd "$(dirname "$0")/.."

map=crates/app-core/tests/fixtures/threat-tests.json
mapfile -t tests < <(jq -r '.[].tests[]' "$map" | sort -u)
echo "${#tests[@]} tests"
cargo test --locked -p app-core --test it -- --exact "${tests[@]}"
