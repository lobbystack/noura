#!/usr/bin/env bash
# Checks sync-protocol, which the desktop client, the sync server and
# account-web share (docs/workspace-format/sync-v2.md §4):
# - its dependency tree, on every target, holds no filesystem, socket or
#   database crate;
# - it builds for wasm32-unknown-unknown, account-web's target, with its
#   wasm-js feature.
# Pass --no-build to check the dependency tree only.
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0

found="$(
  cargo tree --locked --package sync-protocol --edges normal,build --target all --prefix none \
    --format '{p}' | sort -u |
    grep -E '^(tokio|mio|socket2|hyper|h2|reqwest|ureq|curl|isahc|surf|attohttpc|minreq|ehttp|rusqlite|libsqlite3-sys|sqlx|sqlx-core|postgres|tokio-postgres|diesel|sled|redb|notify|walkdir|tempfile|fs2|fs-err|atomic-write-file|async-std|openssl|native-tls|interprocess) v' ||
    true
)"
if [ -n "$found" ]; then
  echo "error: sync-protocol depends on a filesystem, socket or database crate:"
  echo "$found" | sed 's/^/  /'
  fail=1
fi

if [ "${1:-}" != "--no-build" ]; then
  if ! cargo build --locked --package sync-protocol --features wasm-js --target wasm32-unknown-unknown; then
    echo "error: sync-protocol doesn't build for wasm32-unknown-unknown"
    fail=1
  fi
fi

if [ "$fail" = 0 ]; then
  echo "sync-protocol ok"
fi
exit "$fail"
