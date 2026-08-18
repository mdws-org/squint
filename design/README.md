# Icon

`squint-icon.svg` is the source of truth. Edit it, then run `./build-icon.sh` to regenerate `squint.icns`.

The artwork follows the macOS 11 and later icon geometry: an 824 pt squircle with a corner radius of 185.4 pt, centred in a 1024 pt canvas, with the surrounding margin left transparent. Do not fill the margin. The system draws the shadow.

The mark is a cutout. The squircle carries the mass and the eye is the void, which keeps the shape readable at 16 px where a stroked outline thins out and disappears.

If the 16 px and 32 px renders read as muddy in the Finder context menu, draw a simplified glyph for those two sizes rather than scaling the full artwork down. Apple ships per-size artwork for this reason.
