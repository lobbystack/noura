#!/usr/bin/env bash
# Checks the collaboration sandbox (docs/workspace-format/sync-v2.md §22):
# - module: crates/collab-sandbox-guest builds to the exact module that
#   crates/collab-sandbox embeds and sync-v2-collaboration.json pins, so a
#   change to the guest's source, its yrs copy or its build fails until the
#   module, the fixture and the spec change with it;
# - features: the guest's yrs resolves with no feature on, neither
#   `small-client`, which crates/local-core keeps for its v1 sync code, nor
#   `sync` (§3);
# - guest: the guest's own unit tests pass natively;
# - backends: the sandbox's tests, its fixtures and near-limit records
#   included, pass on Pulley as well as Cranelift, with the same verdicts and
#   fuel;
# - release: the previous Noura release reaches the same verdicts and fuel on
#   the current fixture's cases, skipped while no release holds the sandbox.
#
#   scripts/check-collab-sandbox.sh                 # every check
#   scripts/check-collab-sandbox.sh backends        # some of them
#   NOURA_PREVIOUS_RELEASE=<commit> scripts/check-collab-sandbox.sh release
set -euo pipefail

cd "$(dirname "$0")/.."
repo="$PWD"
checks=("$@")
if [ "${#checks[@]}" -eq 0 ]; then
  checks=(module features guest backends release)
fi

module=crates/collab-sandbox/guest/collab_sandbox_guest.wasm
fixture=docs/workspace-format/fixtures/sync-v2-collaboration.json
fail=0

for check in "${checks[@]}"; do
  case "$check" in
    module)
      built="$repo/target/tmp/check-collab-sandbox/collab_sandbox_guest.wasm"
      CARGO_TARGET_DIR="$repo/target/tmp/check-collab-sandbox" \
        crates/collab-sandbox-guest/build.sh "$built"
      if ! cmp -s "$built" "$module"; then
        echo "error: crates/collab-sandbox-guest builds to another module than $module."
        echo "  Rebuild it with crates/collab-sandbox-guest/build.sh, then regenerate"
        echo "  $fixture and update the spec: a new module is a protocol change."
        fail=1
      fi
      ;;
    features)
      resolved="$(
        cd crates/collab-sandbox-guest &&
          cargo tree --locked --target wasm32-unknown-unknown --invert yrs --edges normal \
            --prefix none --format '{p} [{f}]' | head -n 1
      )"
      case "$resolved" in
        "yrs v0.26.0 "*"[]")
          ;;
        *)
          echo "error: the guest's yrs must resolve with no feature on, found: $resolved"
          fail=1
          ;;
      esac
      ;;
    guest)
      if ! (cd crates/collab-sandbox-guest && cargo test --locked --quiet \
        --target-dir "$repo/target/collab-guest-native"); then
        echo "error: the guest's unit tests failed"
        fail=1
      fi
      ;;
    backends)
      if ! cargo test --locked --package collab-sandbox --features pulley; then
        echo "error: the sandbox's tests failed on Cranelift or Pulley"
        fail=1
      fi
      ;;
    release)
      # The newest release tag below HEAD's own, or the commit that
      # NOURA_PREVIOUS_RELEASE names.
      head_tag="$(git describe --tags --exact-match --match 'v*' HEAD 2>/dev/null || true)"
      previous="${NOURA_PREVIOUS_RELEASE:-$(
        git tag --merged HEAD --list 'v*' --sort=-version:refname |
          grep -vx "${head_tag:-^$}" | head -n 1 || true
      )}"
      if [ -z "$previous" ] || ! git cat-file -e "$previous:crates/collab-sandbox/Cargo.toml" 2>/dev/null; then
        echo "no release before this one holds the collaboration sandbox: skipping the release check"
        continue
      fi
      dir="$repo/target/tmp/previous-release"
      rm -rf "$dir"
      git worktree add --detach "$dir" "$previous" >/dev/null
      cp "$fixture" "$dir/$fixture"
      # The previous release pins its own module, so its hash test can't
      # pass against a fixture that pins a newer one; every verdict and fuel
      # count must still agree.
      if ! (cd "$dir" && cargo test --locked --package collab-sandbox --features pulley \
        --target-dir "$repo/target/tmp/previous-release-target" --test fixtures -- \
        --skip the_sandbox_runs_the_module_the_fixture_pins); then
        echo "error: release $previous reaches other verdicts or fuel on the current fixture"
        fail=1
      fi
      git worktree remove --force "$dir"
      ;;
    *)
      echo "error: unknown check $check"
      exit 2
      ;;
  esac
done

if [ "$fail" = 0 ]; then
  echo "collaboration sandbox ok: ${checks[*]}"
fi
exit "$fail"
