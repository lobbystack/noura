#!/usr/bin/env bash
# Checks the editor crates' dependency trees on every target:
# - editor-core has no UI dependency;
# - neither crate depends on Lockbook's own crates, a git source, a mobile
#   host library or an HTTP client;
# - the build holds a single wgpu version.
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0

# Prints each package in a crate's normal dependency tree as `name vX.Y.Z`
# with its source, for every target platform.
tree() {
  cargo tree --locked --package "$1" --edges normal --target all --prefix none \
    --format '{p}' "${@:2}" | sort -u
}

check_absent() {
  local crate="$1" pattern="$2" what="$3"
  shift 3
  local found
  found="$(tree "$crate" "$@" | grep -E "$pattern" || true)"
  if [ -n "$found" ]; then
    echo "error: $crate depends on $what:"
    echo "$found" | sed 's/^/  /'
    fail=1
  fi
}

# editor-core decides text and draws nothing.
check_absent editor-core '^(egui|eframe|epaint|egui-wgpu|egui-winit|winit|wgpu|glyphon|cosmic-text) v' \
  'a UI crate'

for crate in editor-core editor-egui; do
  # Lockbook's core library, its font bundle and its mobile renderer.
  check_absent "$crate" '^(lb-rs|lb-fonts|egui_wgpu_renderer|workspace) v' 'a Lockbook crate'
  # The editor makes no network requests.
  check_absent "$crate" '^(reqwest|hyper|ureq|isahc|surf|curl|attohttpc|minreq|ehttp) v' \
    'an HTTP client'
  # Every dependency comes from crates.io or this repository.
  check_absent "$crate" '\((git\+|https?://)' 'a git source'
  # Lockbook's Android and iOS hosts reached the platform directly.
  check_absent "$crate" '^(jni|ndk|ndk-sys|ndk-context|android-activity|android_logger|objc2-ui-kit|uikit-sys) v' \
    'a mobile host library' --depth 1
done

wgpu_versions="$(grep -c '^name = "wgpu"$' Cargo.lock || true)"
if [ "$wgpu_versions" != "1" ]; then
  echo "error: Cargo.lock holds $wgpu_versions wgpu versions; the editor must paint through eframe's wgpu."
  grep -A1 '^name = "wgpu"$' Cargo.lock | sed 's/^/  /'
  fail=1
fi

if [ "$fail" = 0 ]; then
  echo "editor dependencies ok"
fi
exit "$fail"
