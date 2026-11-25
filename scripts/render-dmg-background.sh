#!/usr/bin/env bash
# Render apps/desktop/packaging/dmg-background.svg into dmg-background.tiff
# beside it: the background of the window Finder shows when Noura's disk
# image opens. The TIFF holds the picture at 1x and at 2x, and Finder draws
# the one that matches the screen.
#
# Needs macOS: AppKit draws the SVG, and tiffutil joins the two sizes.
#
# Usage: scripts/render-dmg-background.sh
set -euo pipefail

cd "$(dirname "$0")/.."

source_svg=apps/desktop/packaging/dmg-background.svg
output=apps/desktop/packaging/dmg-background.tiff

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Draw the SVG at a scale into an sRGB PNG tagged with the matching density,
# 72 dpi at 1x and 144 at 2x, which tiffutil checks.
cat > "$work/render.swift" <<'SWIFT'
import AppKit
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
let scale = CGFloat(Double(arguments[3])!)
guard let image = NSImage(contentsOf: URL(fileURLWithPath: arguments[1])) else {
    FileHandle.standardError.write("Can't read \(arguments[1]).\n".data(using: .utf8)!)
    exit(1)
}
let width = Int((image.size.width * scale).rounded())
let height = Int((image.size.height * scale).rounded())
let context = CGContext(
    data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
context.scaleBy(x: scale, y: scale)
NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
image.draw(in: NSRect(origin: .zero, size: image.size))
let destination = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: arguments[2]) as CFURL, UTType.png.identifier as CFString, 1, nil)!
let dpi = 72 * scale
CGImageDestinationAddImage(
    destination, context.makeImage()!,
    [kCGImagePropertyDPIWidth: dpi, kCGImagePropertyDPIHeight: dpi] as CFDictionary)
guard CGImageDestinationFinalize(destination) else { exit(1) }
SWIFT

swiftc -O "$work/render.swift" -o "$work/render"
"$work/render" "$source_svg" "$work/background.png" 1
"$work/render" "$source_svg" "$work/background@2x.png" 2
tiffutil -cathidpicheck "$work/background.png" "$work/background@2x.png" -out "$output"
echo "Wrote $output"
