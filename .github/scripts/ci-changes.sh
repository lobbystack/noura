#!/usr/bin/env bash
# Decides which CI jobs run, and writes `rust`, `website` and `warm` to
# $GITHUB_OUTPUT as true or false. CONTRIBUTING.md describes the rules.
#
# - A pull request runs the Rust jobs unless it changes only the website or
#   prose, and the website job only when it changes the website. It runs
#   both when its base has no passing CI run, so a change to prose never
#   turns a failing main green.
# - A push to main skips both when a CI run already passed on the same tree,
#   which a pull request's run does when main hasn't moved since. `warm`
#   then asks the build and extras jobs to refill main's caches: most merges
#   change Cargo.lock, which every version bump rewrites, and rust-cache
#   skips the save when main already has the entry.
# - Any other event runs everything.
#
# Needs a checkout of HEAD and its parent, and GH_TOKEN with `actions: read`.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
rust=false
website=false
warm=false

# Whether a changed file can change what the Rust jobs check. Rust code
# reads the format spec, its fixtures, the plugin guides and the notices.
affects_rust() {
  case "$1" in
    docs/workspace-format/* | docs/plugins/* | THIRD_PARTY_NOTICES.md) return 0 ;;
    apps/website/* | docs/* | .agents/* | .claude/* | .opencode/*) return 1 ;;
    package.json | bun.lock | patches/* | prettier.config.js | .prettierignore) return 1 ;;
    */*) return 0 ;;
    *.md) return 1 ;;
    *) return 0 ;;
  esac
}

affects_website() {
  case "$1" in
    apps/website/* | package.json | bun.lock | patches/* | prettier.config.js | .prettierignore) return 0 ;;
    .github/*) return 0 ;;
    *) return 1 ;;
  esac
}

case "${GITHUB_EVENT_NAME:?}" in
  pull_request)
    # HEAD is the merge commit GitHub tests; its first parent is main.
    base="$(git rev-parse HEAD^1)"
    while IFS= read -r file; do
      [ -n "$file" ] || continue
      if affects_rust "$file"; then rust=true; fi
      if affects_website "$file"; then website=true; fi
    done < <(git diff --name-only HEAD^1 HEAD)
    if [ "$rust" = false ] || [ "$website" = false ]; then
      if evidence="$("$here/ci-evidence.sh" "$base")"; then
        echo "The base $base passed CI: $evidence"
      else
        echo "The base $base has no passing CI run, so every job runs."
        rust=true
        website=true
      fi
    fi
    ;;
  push)
    if evidence="$("$here/ci-evidence.sh" "$(git rev-parse HEAD)")"; then
      echo "CI already passed on this tree: $evidence"
      warm=true
    else
      rust=true
      website=true
    fi
    ;;
  *)
    rust=true
    website=true
    ;;
esac

echo "rust=$rust website=$website warm=$warm"
{
  echo "rust=$rust"
  echo "website=$website"
  echo "warm=$warm"
} >> "${GITHUB_OUTPUT:?}"
