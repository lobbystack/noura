#!/usr/bin/env bash
# Regenerates the offline data of sync-store-sqlite's compile-checked
# queries in crates/sync-store-sqlite/.sqlx, so builds need no database.
# With --check it fails instead when the data is stale, as CI runs it.
#
# The script builds a scratch database from the crate's migrations with the
# sqlite3 command-line tool, then compiles the crate against it with
# SQLX_OFFLINE_DIR set, which makes sqlx's macros write their data there.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
crate="$root/crates/sync-store-sqlite"
work="$root/target/sqlx-prepare"

rm -rf "$work"
mkdir -p "$work/offline"
database="$work/schema.sqlite3"
for migration in "$crate"/migrations/*.sql; do
  sqlite3 "$database" < "$migration"
done

# The macros run again only when the crate recompiles.
touch "$crate"/src/*.rs
DATABASE_URL="sqlite://$database" SQLX_OFFLINE_DIR="$work/offline" \
  cargo check -p sync-store-sqlite --locked --quiet

if [ "${1:-}" = "--check" ]; then
  if ! diff -r "$crate/.sqlx" "$work/offline"; then
    echo "::error::The sqlx offline data is stale. Run scripts/sqlx-prepare.sh and commit crates/sync-store-sqlite/.sqlx."
    exit 1
  fi
  echo "The sqlx offline data is current."
else
  rm -rf "$crate/.sqlx"
  cp -R "$work/offline" "$crate/.sqlx"
  echo "Wrote $(ls "$crate/.sqlx" | wc -l | tr -d ' ') query files to crates/sync-store-sqlite/.sqlx."
fi
