#!/usr/bin/env bash
set -euo pipefail

cargo test -p local-core export_bindings --quiet
cargo test -p workspace-format export_bindings --quiet
mkdir -p packages/shared/src/generated
cp crates/local-core/bindings/*.ts packages/shared/src/generated/
cp crates/workspace-format/bindings/*.ts packages/shared/src/generated/
