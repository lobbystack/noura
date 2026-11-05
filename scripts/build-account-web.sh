#!/usr/bin/env bash
# Builds account-web, the WebAssembly module of the sync server's account
# pages, into a folder the server serves when NOURA_ACCOUNT_WEB_DIR names it:
# account_web.js, account_web_bg.wasm and THIRD_PARTY_NOTICES.txt, the
# notice that ships with the module.
#
#   scripts/build-account-web.sh [--debug] [OUT_DIR]
#
# OUT_DIR defaults to target/account-web. The script needs the
# wasm32-unknown-unknown target and the wasm-bindgen command-line tool at
# the version Cargo.lock pins.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
profile=release
if [ "${1:-}" = "--debug" ]; then
  profile=debug
  shift
fi
out="${1:-$root/target/account-web}"

pinned="$(grep -A1 '^name = "wasm-bindgen"$' "$root/Cargo.lock" | sed -n 's/^version = "\(.*\)"$/\1/p')"
installed="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$pinned" != "$installed" ]; then
  echo "error: wasm-bindgen $installed is installed, and Cargo.lock pins $pinned." >&2
  echo "Install it with: cargo install wasm-bindgen-cli --version $pinned --locked" >&2
  exit 1
fi

flags=(--locked -p account-web --lib --target wasm32-unknown-unknown)
if [ "$profile" = release ]; then
  flags+=(--release)
fi
cargo build "${flags[@]}"

mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "$root/target/wasm32-unknown-unknown/$profile/account_web.wasm"
cp "$root/crates/account-web/THIRD_PARTY_NOTICES.txt" "$out/THIRD_PARTY_NOTICES.txt"
echo "Wrote account-web to $out."
