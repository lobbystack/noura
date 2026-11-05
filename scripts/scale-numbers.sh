#!/usr/bin/env bash
# Adds the numbers of a scale or soak run to the workflow's summary.
set -euo pipefail

report=${NOURA_SCALE_REPORT:-numbers.jsonl}
if [[ ! -s "$report" ]]; then
  echo "The run wrote no numbers." >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
  exit 0
fi
{
  jq -r '"### \(.run) on \(.store)"' "$report"
  echo
  echo '| Number | Value |'
  echo '|---|---|'
  jq -r 'to_entries[] | "| \(.key) | \(.value) |"' "$report"
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
