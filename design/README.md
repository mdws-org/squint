# Icon

`squint-icon.svg` is the source of truth. Edit it, then run `./build-icon.sh` to regenerate `squint.icns`.

The artwork follows the macOS 11 and later icon geometry: an 824 pt squircle with a corner radius of 185.4 pt, centred in a 1024 pt canvas, with the surrounding margin left transparent. Do not fill the margin. The system draws the shadow.

The mark is an almost-closed eye: a white opening between a heavier upper lid and a lower lid, on a slate tile inside a dark rim. It has no pupil. At 16 px the opening is about two pixels tall and reads as one white bar between two dark ones, and a pupil broke that bar into two specks at that size.

It was chosen from six rounds of candidates. Each round had to pass two checks before anything was chosen. First, a reader who had seen nothing else was shown each mark at 128 px and asked what it depicts, and only marks named as an eye went on. Second, two reviewers who had not seen how the marks were made ranked the survivors by polish against six shipping Apple icons, rendered at 256, 128, 32 and 16 px on dark and light backgrounds.

If the 16 px and 32 px renders read as muddy in the Finder context menu, draw a simplified glyph for those two sizes rather than scaling the full artwork down. Apple ships per-size artwork for this reason.
