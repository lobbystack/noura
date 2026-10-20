#!/usr/bin/env bash
# Builds the trial S7 hello plugins that plugin-host's trial tests load: the
# Rust plugin for wasm32-wasip2, and the TypeScript plugin with jco under Node.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../../.." && pwd)"

cargo build --manifest-path "$repo/Cargo.toml" --package s7-rust-hello \
  --release --target wasm32-wasip2 --locked

cd "$repo/sdk/typescript/s7-hello"
npm ci --no-audit --no-fund
npm run build
