#!/usr/bin/env bash
# Regenerate the Noura app, favicon, and social assets from the brand source.
#
# The brand source is the N glyph on a flat background. This script keys that
# background out, keeps the glyph, and builds the platform assets:
#
#   * a layered `AppIcon.icon` for macOS 26 and later, compiled to `Assets.car`
#     with `actool`, so the system draws the squircle, background, and
#     light/dark/tinted appearances
#   * full-bleed PNG and Windows icons for the desktop packages, in
#     apps/desktop/icons
#   * favicon, apple-touch-icon, and logo for the website
#
# Requires ImageMagick, and the Apple toolchain (`actool`) for the macOS icon.
# Without `actool`, macOS falls back to a flat icon.icns built with
# `iconutil`, or keeps the checked-in one.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_png="$root/assets/brand/noura-logo.png"
icons_dir="$root/apps/desktop/icons"
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
master="$root/assets/brand/noura-icon-1024.png"
for size in 32 64 128; do
	magick "$master" -resize "${size}x${size}" "$icons_dir/${size}x${size}.png"
done
magick "$master" -resize 256x256 "$icons_dir/128x128@2x.png"
magick "$master" -define icon:auto-resize=256,128,64,48,32,16 "$icons_dir/icon.ico"

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
	cp "$work/iconcar/Assets.car" "$icons_dir/Assets.car"
	cp "$work/iconcar/AppIcon.icns" "$icons_dir/icon.icns"
elif command -v iconutil >/dev/null; then
	echo "actool not found; macOS falls back to the flat icon.icns" >&2
	iconset="$work/icon.iconset"
	mkdir -p "$iconset"
	for size in 16 32 128 256 512; do
		magick "$master" -resize "${size}x${size}" "$iconset/icon_${size}x${size}.png"
		double=$((size * 2))
		magick "$master" -resize "${double}x${double}" "$iconset/icon_${size}x${size}@2x.png"
	done
	iconutil -c icns "$iconset" -o "$icons_dir/icon.icns"
else
	echo "actool and iconutil not found; keeping the checked-in icon.icns" >&2
fi

# 4. Web assets: the glyph on transparency.
web="$root/apps/website/static"
magick "$master" -resize 48x48 "$web/favicon.png"
magick "$master" -resize 180x180 "$web/apple-touch-icon.png"
magick "$master" -resize 256x256 "$web/logo.png"

echo "Assets regenerated from $source_png"
