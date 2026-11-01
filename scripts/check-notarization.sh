#!/usr/bin/env bash
# Check that macOS accepts a signed and notarized Noura disk image: the image
# and the app inside it each carry a stapled notarization ticket, the app's
# signature holds, and Gatekeeper accepts both. An ad hoc build fails here.
#
# Usage: scripts/check-notarization.sh <file.dmg>
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 <file.dmg>" >&2
  exit 2
fi
dmg="$1"

fail() {
  echo "::error::$1" >&2
  exit 1
}

xcrun stapler validate "$dmg" || fail "$dmg has no notarization ticket."
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg" ||
  fail "Gatekeeper refuses $dmg."

mount="$(mktemp -d)"
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg"
trap 'hdiutil detach "$mount"' EXIT

app="$mount/Noura.app"
codesign --verify --deep --strict --verbose=2 "$app" || fail "Noura.app's signature doesn't hold."
xcrun stapler validate "$app" || fail "Noura.app has no notarization ticket."
spctl --assess --type execute --verbose=2 "$app" || fail "Gatekeeper refuses Noura.app."
echo "Gatekeeper accepts $dmg and Noura.app."
