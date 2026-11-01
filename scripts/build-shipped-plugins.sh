#!/usr/bin/env bash
# Build the first-party plugins every package ships, as optimized WebAssembly
# components, into apps/desktop/packaging/plugins. Each gets a folder named
# after its ID, holding its manifest and its component, which is the form
# Noura installs from. cargo-packager copies that folder into the app's
# resources, and bundled_plugins() in apps/desktop finds it there.
#
# Needs the wasm32-wasip2 Rust target. app_core::SHIPPED_PLUGINS lists the
# same plugins, and the app's --smoke run fails on one it can't find.
set -euo pipefail

cd "$(dirname "$0")/.."

plugins=(ai calendar projects tasks)
out=apps/desktop/packaging/plugins

packages=()
for id in "${plugins[@]}"; do packages+=(--package "$id-plugin"); done
# Debug info would make each component far larger.
CARGO_PROFILE_RELEASE_STRIP=debuginfo \
  cargo build --release --locked --target wasm32-wasip2 "${packages[@]}"

rm -rf "$out"
for id in "${plugins[@]}"; do
  mkdir -p "$out/$id"
  cp "crates/plugins/$id/plugin.toml" "$out/$id/"
  cp "target/wasm32-wasip2/release/${id}_plugin.wasm" "$out/$id/"
done
