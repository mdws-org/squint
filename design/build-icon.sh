#!/bin/sh
# Build squint.icns from squint-icon.svg.
# Requires inkscape (vector rasterizer) and iconutil (macOS built-in).
set -eu
cd "$(dirname "$0")"

command -v inkscape >/dev/null || { echo "inkscape not found. Install it with: brew install inkscape" >&2; exit 1; }

rm -rf squint.iconset png
mkdir -p squint.iconset png

for s in 16 32 64 128 256 512 1024; do
    inkscape -w "$s" -h "$s" -o "png/squint-${s}.png" squint-icon.svg >/dev/null 2>&1
done

cp png/squint-16.png   squint.iconset/icon_16x16.png
cp png/squint-32.png   squint.iconset/icon_16x16@2x.png
cp png/squint-32.png   squint.iconset/icon_32x32.png
cp png/squint-64.png   squint.iconset/icon_32x32@2x.png
cp png/squint-128.png  squint.iconset/icon_128x128.png
cp png/squint-256.png  squint.iconset/icon_128x128@2x.png
cp png/squint-256.png  squint.iconset/icon_256x256.png
cp png/squint-512.png  squint.iconset/icon_256x256@2x.png
cp png/squint-512.png  squint.iconset/icon_512x512.png
cp png/squint-1024.png squint.iconset/icon_512x512@2x.png

iconutil -c icns squint.iconset -o squint.icns
rm -rf squint.iconset png
echo "built squint.icns"
