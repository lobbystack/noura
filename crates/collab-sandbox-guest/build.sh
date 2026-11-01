#!/usr/bin/env bash
# Builds the collaboration sandbox's guest module for wasm32-unknown-unknown
# and copies it to crates/collab-sandbox/guest/collab_sandbox_guest.wasm, the
# module the app runs and whose BLAKE3 hash sync-v2-collaboration.json pins.
#
#   crates/collab-sandbox-guest/build.sh          # rebuild the pinned module
#   crates/collab-sandbox-guest/build.sh OUT.wasm # build a copy elsewhere
#
# The build is reproducible: the guest's own Cargo.lock pins every crate,
# rust-toolchain.toml pins the compiler, and the remapped paths keep the
# machine's directories out of the module. Cargo hashes the full path of
# sync-protocol, a path dependency outside the guest's workspace, into its
# symbols, and those symbols order the module's functions, so the build runs
# on a copy at one fixed path. The compiler's host still changes the order
# of the module's functions, so the pinned module comes from x86-64 Linux, as
# in CI; elsewhere, run this in a linux/amd64 container. getrandom's "unsupported"
# backend keeps sync-protocol's browser randomness, and its wasm-bindgen
# imports, out of the module, which may import nothing.
set -euo pipefail

guest="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$guest/../.." && pwd)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
# The build runs from the guest's folder, so relative paths resolve here.
absolute() {
  case "$1" in
    /*) echo "$1" ;;
    *) echo "$PWD/$1" ;;
  esac
}
target_dir="$(absolute "${CARGO_TARGET_DIR:-$repo/target}")/collab-sandbox-guest"
out="$(absolute "${1:-$repo/crates/collab-sandbox/guest/collab_sandbox_guest.wasm}")"

source_copy=/tmp/noura-collab-sandbox-guest
rm -rf "${source_copy:?}"
mkdir -p "$source_copy"
tar -C "$repo" --exclude target -cf - Cargo.toml crates/collab-sandbox-guest crates/sync-protocol |
  tar -xf - -C "$source_copy"

flags=(
  --cfg 'getrandom_backend="unsupported"'
  "--remap-path-prefix=$cargo_home=/cargo"
  "--remap-path-prefix=$source_copy=/noura"
)
CARGO_ENCODED_RUSTFLAGS="$(IFS=$'\x1f'; echo "${flags[*]}")"
export CARGO_ENCODED_RUSTFLAGS

(
  cd "$guest"
  cargo build --quiet --locked --release --target wasm32-unknown-unknown \
    --manifest-path "$source_copy/crates/collab-sandbox-guest/Cargo.toml" \
    --target-dir "$target_dir"
)
mkdir -p "$(dirname "$out")"
cp "$target_dir/wasm32-unknown-unknown/release/collab_sandbox_guest.wasm" "$out"
