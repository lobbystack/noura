#!/usr/bin/env bash
# Print the updater feed, latest.json, for a release.
#
# Usage: scripts/write-update-feed.sh <version> <download URL> <folder>
#
# The folder holds the release's update bundles, each with the .sig file
# cargo-packager wrote beside it: the macOS app archive (*.app.tar.gz), the
# Windows installer (*.msi) and the Linux AppImage (*.AppImage). The
# download URL is where the release serves those files. The desktop app's
# updater reads the feed; apps/desktop/src/updates.rs tests the same shape.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "Usage: $0 <version> <download URL> <folder>" >&2
  exit 2
fi
version="$1"
base="${2%/}"
folder="$3"

one() {
  local matches=("$folder"/$1)
  if [ "${#matches[@]}" -ne 1 ] || [ ! -f "${matches[0]}" ] || [ ! -f "${matches[0]}.sig" ]; then
    echo "Expected one signed file matching $1 in $folder." >&2
    exit 1
  fi
  echo "${matches[0]}"
}

# One platform's entry: where to download the bundle, its signature and how
# the updater installs it.
entry() {
  jq -n \
    --arg url "$base/$(basename "$1")" \
    --arg signature "$(cat "$1.sig")" \
    --arg format "$2" \
    '{url: $url, signature: $signature, format: $format}'
}

macos_bundle="$(one '*.app.tar.gz')"
windows_bundle="$(one '*.msi')"
linux_bundle="$(one '*.AppImage')"
macos="$(entry "$macos_bundle" app)"
windows="$(entry "$windows_bundle" wix)"
linux="$(entry "$linux_bundle" appimage)"

# The macOS app is universal, so both architectures get the same bundle.
jq -n \
  --arg version "$version" \
  --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson macos "$macos" \
  --argjson windows "$windows" \
  --argjson linux "$linux" \
  '{
    version: $version,
    pub_date: $date,
    platforms: {
      "macos-aarch64": $macos,
      "macos-x86_64": $macos,
      "windows-x86_64": $windows,
      "linux-x86_64": $linux
    }
  }'
