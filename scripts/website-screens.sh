#!/usr/bin/env bash
# Draw the website's images from the real app and write them where the site
# serves them:
#
# - apps/website/static/screens/{files,tasks,calendar,ai}.webp, whole
#   windows at 2400 by 1560 pixels
# - apps/website/static/og.png, the 1200 by 630 social card
# - apps/website/static/parts/<part>-{light,dark}.webp, pieces of the
#   interface for the feature cards, with a README.md listing their sizes.
#   ai-dialog keeps its transparent background.
#
# The ignored tests renders::website_screens and renders::website_parts in
# crates/app-ui draw them through wgpu, so this needs a GPU or a software
# Vulkan driver. website_parts puts its workspace at /Users/Shared/Studio on
# a Mac, so Settings shows a path like a person's, and removes it after; it
# stops if that folder exists already. This also needs cwebp and
# ImageMagick: `brew install webp imagemagick`.
set -euo pipefail

cd "$(dirname "$0")/.."

for tool in cwebp magick; do
  if ! command -v "$tool" >/dev/null; then
    echo "error: $tool is missing. Install it with: brew install webp imagemagick" >&2
    exit 1
  fi
done

renders="$(mktemp -d)"
trap 'rm -rf "$renders"' EXIT

# The tests load the plugins this script builds, without debug info.
crates/plugin-sdk/build-plugins.sh
NOURA_RENDERS="$renders" NOURA_RENDER_SCALE=2 NOURA_REQUIRE_PLUGINS=1 \
  cargo test -p app-ui --test it --locked renders::website_ -- --ignored --test-threads=1

# Lossless keeps text sharp, and the interface's flat colors make it smaller
# than lossy quality 90.
webp() {
  cwebp -quiet -lossless -z 9 "$1" -o "$2"
}

screens=apps/website/static/screens
mkdir -p "$screens"
for screen in files tasks calendar ai; do
  webp "$renders/$screen.png" "$screens/$screen.webp"
done

# The social card: the Files screen with rounded corners and a soft shadow on
# a lavender gradient, running off the card's bottom edge. 256 colors would
# band the gradient and come out larger than true color.
width=1000
left=100
top=60
blur=28
magick "$renders/files.png" -resize "${width}x" "$renders/card-shot.png"
height="$(magick identify -format '%h' "$renders/card-shot.png")"
magick "$renders/card-shot.png" \
  \( -size "${width}x${height}" xc:black -fill white \
     -draw "roundrectangle 0,0,$((width - 1)),$((height - 1)),14,14" \) \
  -alpha off -compose CopyOpacity -composite "$renders/card-rounded.png"
magick "$renders/card-rounded.png" -background '#2b2170' -shadow "22x${blur}+0+18" \
  "$renders/card-shadow.png"
# The shadow reaches twice its blur past each side of the screenshot.
magick -size 1200x630 -define gradient:direction=SouthEast gradient:'#f2f0ff-#dedcff' \
  "$renders/card-shadow.png" -geometry "+$((left - 2 * blur))+$((top - 2 * blur + 18))" -composite \
  "$renders/card-rounded.png" -geometry "+${left}+${top}" -composite \
  -alpha off -strip -define png:compression-level=9 -define png:compression-strategy=0 \
  PNG24:apps/website/static/og.png

# What each part shows, for the README.
shows() {
  case "$1" in
    ai-approval) echo "A question about the workspace, the answer, and Noura asking before the AI creates a task" ;;
    ai-dialog) echo "Noura's prompt before the AI creates a task, alone with its shadow on a transparent background" ;;
    plugins) echo "Settings > Plugins, with Dictation and Transcription turned off" ;;
    plugins-compact) echo "The same plugins in a narrow window, each switch close to its name, for phones" ;;
    task-rows) echo "The Studio admin project's tasks: three upcoming and one done" ;;
    file-tree) echo "The sidebar's files, with three folders open and the Website relaunch note selected" ;;
    mcp) echo "The MCP configuration in Settings > External tools, with its Copy button" ;;
    palette) echo "Search for \"journal\", matching file names and contents" ;;
    calendar-day) echo "Monday in the Calendar's day view, with a task due that day" ;;
  esac
}

parts=apps/website/static/parts
mkdir -p "$parts"
{
  echo "# Interface parts"
  echo
  echo "\`scripts/website-screens.sh\` crops these pieces from the app's real interface at 2 pixels per point. Each part has a \`-light.webp\` and a \`-dark.webp\` file of the same size."
  echo
  echo "| Part | Pixels | Shows |"
  echo "| --- | --- | --- |"
  for part in ai-approval ai-dialog plugins plugins-compact task-rows file-tree mcp palette calendar-day; do
    for theme in light dark; do
      webp "$renders/$part-$theme.png" "$parts/$part-$theme.webp"
    done
    size="$(magick identify -format '%w × %h' "$renders/$part-light.png")"
    echo "| \`$part\` | $size | $(shows "$part") |"
  done
} > "$parts/README.md"

ls -l "$screens"/*.webp apps/website/static/og.png "$parts"/*.webp
