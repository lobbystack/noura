#!/usr/bin/env bash
# Regenerates the offline data of the stores' compile-checked queries, in
# crates/sync-store-sqlite/.sqlx and crates/sync-store-postgres/.sqlx, so
# builds need no database. With --check it fails instead when the data is
# stale, as CI runs it.
#
# For each store the script builds a scratch schema from the crate's
# migrations, then compiles the crate against it with SQLX_OFFLINE_DIR set,
# which makes sqlx's macros write their data there. The sqlite3
# command-line tool builds SQLite's schema. PostgreSQL's goes into the empty
# database that SQLX_POSTGRES_URL names, or else into a throwaway cluster
# that initdb and pg_ctl start on a loopback port and that stops when the
# script ends. NOURA_POSTGRES_BIN, or else pg_config --bindir, or else the
# PATH, names PostgreSQL's bin directory.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$root/target/sqlx-prepare"
check=false
if [ "${1:-}" = "--check" ]; then
  check=true
fi

rm -rf "$work"
mkdir -p "$work"
stale=false

# Compiles crates/$1 against the database at $2 and compares or copies its
# offline data.
prepare() {
  local name="$1" url="$2"
  local crate="$root/crates/$name" offline="$work/$name"
  mkdir -p "$offline"
  # The macros run again only when the crate recompiles.
  touch "$crate"/src/*.rs
  DATABASE_URL="$url" SQLX_OFFLINE_DIR="$offline" \
    cargo check -p "$name" --locked --quiet
  if $check; then
    if ! diff -r "$crate/.sqlx" "$offline"; then
      echo "::error::The sqlx offline data of $name is stale. Run scripts/sqlx-prepare.sh and commit crates/$name/.sqlx."
      stale=true
    fi
  else
    rm -rf "$crate/.sqlx"
    cp -R "$offline" "$crate/.sqlx"
    echo "Wrote $(ls "$crate/.sqlx" | wc -l | tr -d ' ') query files to crates/$name/.sqlx."
  fi
}

# SQLite.
database="$work/schema.sqlite3"
for migration in "$root"/crates/sync-store-sqlite/migrations/*.sql; do
  sqlite3 "$database" < "$migration"
done
prepare sync-store-sqlite "sqlite://$database"

# PostgreSQL.
bin="${NOURA_POSTGRES_BIN:-}"
if [ -z "$bin" ] && command -v pg_config > /dev/null; then
  bin="$(pg_config --bindir)"
fi
if [ -z "$bin" ] && command -v psql > /dev/null; then
  bin="$(dirname "$(command -v psql)")"
fi
if [ -z "$bin" ] || [ ! -x "$bin/psql" ]; then
  echo "::error::PostgreSQL's psql wasn't found. Install PostgreSQL or set NOURA_POSTGRES_BIN."
  exit 1
fi
url="${SQLX_POSTGRES_URL:-}"
if [ -z "$url" ]; then
  cluster="$work/postgres"
  LC_ALL=C "$bin/initdb" -D "$cluster" -A trust -U postgres -E UTF8 --locale=C --no-sync > /dev/null
  started=false
  for _ in 1 2 3 4 5; do
    port=$(( 20000 + RANDOM % 40000 ))
    {
      echo "port = $port"
      echo "listen_addresses = '127.0.0.1'"
      echo "unix_socket_directories = ''"
    } >> "$cluster/postgresql.conf"
    if LC_ALL=C "$bin/pg_ctl" -D "$cluster" -l "$work/postgres.log" -w start > /dev/null; then
      started=true
      break
    fi
  done
  if ! $started; then
    cat "$work/postgres.log"
    echo "::error::The throwaway PostgreSQL cluster didn't start."
    exit 1
  fi
  trap 'LC_ALL=C "$bin/pg_ctl" -D "$cluster" -m immediate -w stop > /dev/null' EXIT
  url="postgres://postgres@127.0.0.1:$port/postgres"
fi
for migration in "$root"/crates/sync-store-postgres/migrations/*.sql; do
  "$bin/psql" -q -v ON_ERROR_STOP=1 "$url" -f "$migration" > /dev/null
done
prepare sync-store-postgres "$url"

if $stale; then
  exit 1
fi
if $check; then
  echo "The sqlx offline data is current."
fi
