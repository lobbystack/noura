#!/usr/bin/env bash
# Regenerate the Noura app, favicon, and social assets from the brand source.
#
# The brand source is the N glyph on a flat background. This script keys that
# background out, keeps the glyph, and builds the platform assets:
#
#   * a layered `AppIcon.icon` for macOS 26 and later, compiled to `Assets.car`
#     with `actool`, so the system draws the squircle, background, and
#     light/dark/tinted appearances
#   * full-bleed icons for iOS, Android, Windows, and Linux via `tauri icon`
#   * favicon, apple-touch-icon, and the in-app logo for the web apps
#
# Requires ImageMagick, and the Apple toolchain (`actool`) for the macOS icon.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_png="$root/assets/brand/noura-logo.png"
icons_dir="$root/src-tauri/icons"
app_icon="$root/assets/macos/AppIcon.icon"
source_background='#FDFAFD'

if [ ! -f "$source_png" ]; then
	echo "missing brand source: $source_png" >&2
	exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 1. Key the flat background out of the glyph.
magick "$source_png" -fuzz 10% -transparent "$source_background" "$work/glyph.png"

# 2. Full-bleed master for non-Apple platforms and the web. Fit the glyph
#    inside ~75% of the canvas so it keeps a clear margin.
magick "$work/glyph.png" -trim +repage -resize 760x760 "$work/glyph-fit.png"
magick -size 1024x1024 xc:none "$work/glyph-fit.png" -gravity center \
	-composite "$root/assets/brand/noura-icon-1024.png"
(cd "$root" && bun run tauri icon "$root/assets/brand/noura-icon-1024.png")

# 3. macOS layered icon: the glyph on the Apple grid, no mask and no baked
#    background, so the system supplies both.
mkdir -p "$app_icon/Assets"
magick "$work/glyph.png" -trim +repage -resize 800x800 "$work/glyph-macos.png"
magick -size 1024x1024 xc:none "$work/glyph-macos.png" -gravity center \
	-composite "$app_icon/Assets/noura-n.png"
cat > "$app_icon/icon.json" <<'JSON'
{
	"groups": [
		{
			"layers": [
				{
					"image-name": "noura-n.png",
					"name": "N"
				}
			],
			"name": "N"
		}
	],
	"supported-platforms": {
		"squares": ["macOS"]
	}
}
JSON

actool="$(xcrun --find actool 2>/dev/null || true)"
if [ -n "$actool" ]; then
	mkdir -p "$work/iconcar"
	"$actool" "$app_icon" --compile "$work/iconcar" --platform macosx \
		--minimum-deployment-target 15.0 --app-icon AppIcon \
		--include-all-app-icons \
		--output-partial-info-plist "$work/iconcar/partial.plist" >/dev/null
	cp "$work/iconcar/Assets.car" "$root/src-tauri/Assets.car"
	cp "$work/iconcar/AppIcon.icns" "$icons_dir/icon.icns"
else
	echo "actool not found; macOS falls back to the flat icon.icns" >&2
fi

# 4. Web assets: the glyph on transparency.
for dest in "$root/apps/app/static" "$root/apps/website/static"; do
	magick "$root/assets/brand/noura-icon-1024.png" -resize 48x48 "$dest/favicon.png"
	magick "$root/assets/brand/noura-icon-1024.png" -resize 180x180 "$dest/apple-touch-icon.png"
	magick "$root/assets/brand/noura-icon-1024.png" -resize 256x256 "$dest/logo.png"
done

echo "Assets regenerated from $source_png"
