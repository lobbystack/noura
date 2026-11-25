#!/usr/bin/env bash
# Build the desktop app in release mode and package it with cargo-packager
# for this operating system: an app bundle and a .dmg on macOS, an .msi on
# Windows, and a .deb and an AppImage on Linux. Packages land in
# target/<triple>/release or target/release.
#
# Needs cargo-packager and cargo-about on PATH, and the wasm32-wasip2 Rust
# target. cargo-about writes the third-party license list every package
# ships, and scripts/build-shipped-plugins.sh builds the plugins each one
# ships.
#
# Usage: scripts/package-desktop.sh [--universal]
#   --universal  On macOS, build one app for Apple silicon and Intel.
#
# Set CARGO_PACKAGER_SIGN_PRIVATE_KEY (and its _PASSWORD) to sign each
# package for the updater; cargo-packager writes a .sig beside each one.
set -euo pipefail

cd "$(dirname "$0")/.."

universal=false
for argument in "$@"; do
  case "$argument" in
    --universal) universal=true ;;
    *)
      echo "Unknown option: $argument" >&2
      exit 2
      ;;
  esac
done

host="$(rustc -vV | sed -n 's/^host: //p')"
packager=(cargo packager)
case "$host" in
  *-apple-darwin)
    formats=app,dmg
    # cargo-packager has Finder lay out the disk image's window, with the
    # background, icon positions and size from apps/desktop/Cargo.toml, but
    # skips that when CI is "true", as GitHub Actions sets it. Without it the
    # image opens as a plain window. GitHub's macOS runners have a desktop
    # session that runs Finder, so let it run there too.
    packager=(env -u CI cargo packager)
    ;;
  *-windows-msvc) formats=wix ;;
  *-linux-gnu) formats=deb,appimage ;;
  *)
    echo "No packages for $host." >&2
    exit 1
    ;;
esac

licenses=apps/desktop/packaging/THIRD_PARTY_LICENSES.html
generate_licenses() {
  local targets=()
  for target in "$@"; do targets+=(--target "$target"); done
  cargo about generate --locked --fail \
    --manifest-path apps/desktop/Cargo.toml \
    --config apps/desktop/packaging/about.toml \
    "${targets[@]}" \
    --output-file "$licenses" \
    apps/desktop/packaging/about.hbs
}

if [ "$universal" = true ] && [[ "$host" != *-apple-darwin ]]; then
  echo "--universal works only on macOS." >&2
  exit 2
fi

scripts/build-shipped-plugins.sh

if [ "$universal" = true ]; then
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
  generate_licenses "${targets[@]}"
  for target in "${targets[@]}"; do
    cargo build --release --locked -p desktop --target "$target"
  done
  mkdir -p target/universal-apple-darwin/release
  lipo -create -output target/universal-apple-darwin/release/noura \
    target/aarch64-apple-darwin/release/noura \
    target/x86_64-apple-darwin/release/noura
  "${packager[@]}" --release -p desktop --formats "$formats" --target universal-apple-darwin
else
  generate_licenses "$host"
  cargo build --release --locked -p desktop
  "${packager[@]}" --release -p desktop --formats "$formats"
fi
