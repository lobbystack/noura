#!/usr/bin/env bash
# Builds the Rust plugins that the plugin tests load: the hello plugin and
# plugin-host's test plugins, as WebAssembly components for wasm32-wasip2.
# They land in target/wasm32-wasip2/debug/, without debug info, which would
# make each component about 20 times larger.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"

CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_DEV_STRIP=debuginfo \
  cargo build --manifest-path "$repo/Cargo.toml" --target wasm32-wasip2 --locked \
  --package hello-plugin --package probe-plugin --package recipes-plugin \
  --package uses-files-plugin --package misbehave-plugin
