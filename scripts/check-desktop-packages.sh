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

# macOS has no timeout command.
time_limit=(timeout 180)
command -v timeout > /dev/null || time_limit=(perl -e 'alarm 180; exec @ARGV')

# Windows: the launch has hung past every limit that waits on the process or
# on a pipe it holds, so it runs in the background, writes to a file, and this
# only polls. A hang gets a report and never waits for the process to end.
launch_on_windows() {
  local output seconds=0
  output="$(mktemp)"
  "$1" --smoke > "$output" 2>&1 < /dev/null &
  local pid=$!
  while [ "$seconds" -lt 150 ]; do
    if ! kill -0 "$pid" 2> /dev/null; then
      wait "$pid"
      local status=$?
      cat "$output"
      return "$status"
    fi
    sleep 1
    seconds=$((seconds + 1))
  done
  echo "Still running after $seconds seconds."
  cat "$output"
  powershell -NoProfile -Command "
    Get-Process noura -ErrorAction SilentlyContinue | ForEach-Object {
      'Process {0}: responding {1}, window [{2}], {3} threads' -f \$_.Id, \$_.Responding, \$_.MainWindowTitle, \$_.Threads.Count
      \$_.Modules | ForEach-Object { \$_.ModuleName } | Select-Object -Last 15
    }
  " < /dev/null
  taskkill //F //T //IM noura.exe < /dev/null > /dev/null 2>&1 || true
  return 1
}

launch() {
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) launch_on_windows "$1" ;;
    *) "${time_limit[@]}" "$@" --smoke ;;
  esac
}

print_app_logs() {
  local log
  find "${LOCALAPPDATA:-$HOME}/noura" -iname 'noura.log*' -type f 2> /dev/null | while read -r log; do
    echo "--- $log"
    tail -n 60 "$log"
  done
}

# Run the app with --smoke and check its report. $2 is "on" or "off".
check() {
  local name="$1" updates="$2"
  shift 2
  local report
  echo "Launching $name."
  report="$(RUST_LOG=info launch "$@")" || {
    echo "$name: $report"
    print_app_logs
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
    echo "Installing $(basename "$msi")."
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
