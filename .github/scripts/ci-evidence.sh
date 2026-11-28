#!/usr/bin/env bash
# Finds a successful CI run that tested the same tree as a commit, and
# prints its URL. Exits 1 when no such run exists.
#
#   .github/scripts/ci-evidence.sh <commit>
#
# A run counts when ci.yml in this repository, not in a fork, finished it
# with success, and either:
# - its `CI passed` job recorded the commit's tree as the artifact
#   ci-tree-<tree>; a pull request's run records the tree of the merge
#   commit it tested, which a squash merge reproduces when main hasn't
#   moved since;
# - it ran on a push of the commit itself, as runs from before the
#   artifacts did.
#
# Needs GH_TOKEN with `actions: read` and `contents: read`, and
# GITHUB_REPOSITORY.
set -euo pipefail

repo="${GITHUB_REPOSITORY:?}"

read -r commit tree < <(gh api "repos/$repo/commits/$1" --jq '"\(.sha) \(.commit.tree.sha)"')

runs="$(
  gh api -X GET "repos/$repo/actions/artifacts" -f name="ci-tree-$tree" -f per_page=100 \
    --jq '.artifacts[]
      | select(.expired | not)
      | select(.workflow_run.head_repository_id == .workflow_run.repository_id)
      | .workflow_run.id' | sort -u
)"
for run in $runs; do
  url="$(
    gh api "repos/$repo/actions/runs/$run" \
      --jq 'select(.path == ".github/workflows/ci.yml" and .conclusion == "success") | .html_url'
  )"
  if [ -n "$url" ]; then
    echo "$url"
    exit 0
  fi
done

url="$(
  gh api -X GET "repos/$repo/actions/runs" -f head_sha="$commit" -f event=push \
    -f status=success -f per_page=100 \
    --jq '[.workflow_runs[] | select(.path == ".github/workflows/ci.yml")][0].html_url // empty'
)"
if [ -n "$url" ]; then
  echo "$url"
  exit 0
fi
exit 1
