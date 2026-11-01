#!/usr/bin/env bash
# Install or mount each package scripts/package-desktop.sh built, launch the
# installed app with --smoke and check what it reports: its first window,
# the bundled license list, the plugins it ships, and whether it updates
# itself. --smoke itself fails when a plugin is missing.
#
# macOS mounts the .dmg and copies the app out, Windows installs the .msi,
# and Linux installs the .deb and runs the AppImage. Linux needs a display:
# run this under xvfb-run.
set -euo pipefail

cd "$(dirname "$0")/.."

# Every directory cargo-packager may have written packages to.
package_dirs=(target/release target/*/release)

find_one() {
  local pattern="$1" matches=()
  for dir in "${package_dirs[@]}"; do
    for file in $dir/$pattern; do
      [ -e "$file" ] && matches+=("$file")
    done
  done
  if [ "${#matches[@]}" -ne 1 ]; then
    echo "Expected one package matching $pattern, found ${#matches[@]}." >&2
    exit 1
  fi
  echo "${matches[0]}"
}

# Run the app with --smoke and check its report. $2 is "on" or "off".
check() {
  local name="$1" updates="$2"
  shift 2
  local report
  report="$("$@" --smoke)" || {
    echo "$name: $report"
    echo "::error::$name failed its smoke run."
    exit 1
  }
  echo "$name: $report"
  grep -q "drew its first window." <<<"$report" || {
    echo "::error::$name didn't draw its first window."
    exit 1
  }
  if grep -q "^Third-party licenses: missing" <<<"$report"; then
    echo "::error::$name doesn't ship THIRD_PARTY_LICENSES.html."
    exit 1
  fi
  grep -q "^Plugins: AI, Calendar, Projects, Tasks$" <<<"$report" || {
    echo "::error::$name doesn't ship Tasks, Projects, Calendar and AI."
    exit 1
  }
  grep -q "^Updates: $updates" <<<"$report" || {
    echo "::error::$name should have updates $updates."
    exit 1
  }
}

work="$(mktemp -d)"
case "$(uname -s)" in
  Darwin)
    dmg="$(find_one '*.dmg')"
    mkdir "$work/mount" "$work/Applications"
    # The image carries a license agreement, and hdiutil cancels the mount
    # unless something answers it.
    PAGER=cat hdiutil attach -nobrowse -readonly -mountpoint "$work/mount" "$dmg" < <(yes)
    cp -R "$work/mount/Noura.app" "$work/Applications/"
    hdiutil detach "$work/mount"
    check "$(basename "$dmg")" on "$work/Applications/Noura.app/Contents/MacOS/noura"
    ;;
  MINGW* | MSYS* | CYGWIN*)
    msi="$(find_one '*.msi')"
    msi_windows="$(cygpath -w "$msi")"
    powershell -NoProfile -Command \
      "\$p = Start-Process msiexec -ArgumentList '/i', '\"$msi_windows\"', '/quiet', '/norestart' -Wait -PassThru; exit \$p.ExitCode"
    check "$(basename "$msi")" on "$(cygpath -u "$PROGRAMFILES")/Noura/noura.exe"
    ;;
  Linux)
    deb="$(find_one '*.deb')"
    sudo apt-get install -y "./$deb"
    check "$(basename "$deb")" off /usr/bin/noura
    appimage="$(find_one '*.AppImage')"
    chmod +x "$appimage"
    check "$(basename "$appimage")" on "./$appimage"
    ;;
  *)
    echo "No packages to check on $(uname -s)." >&2
    exit 1
    ;;
esac
