# squint

A macOS image optimizer that compresses to a perceptual target instead of a fixed quality number.

Most optimizers ask you to choose a quality setting once and then apply it to every image forever. A flat interface screenshot and a noisy photograph do not tolerate the same setting, so one number wastes bytes on the first and visibly damages the second. squint measures perceived difference per image and finds the smallest file that stays under the threshold you set. You get consistent perceived quality rather than a consistent encoder setting.

## Status

Pre-alpha. No code exists yet. This repository currently holds the design and the roadmap.

## Why this exists

[ImageOptim](https://imageoptim.com) has been the reference tool in this space for over a decade, and squint owes its interaction model to it: drop files on a window, or right-click them in Finder, and they get smaller. That part is worth preserving exactly.

Three things are missing from it. It cannot write WebP or AVIF. It produces exactly one output per input, because its job model tracks a single result per file. And its quality is a fixed number applied uniformly, so the result varies in perceived quality from image to image.

One behaviour is worth correcting rather than copying. With strip-metadata enabled, ImageOptim passes `-copy none` to jpegtran and `--strip-all` to jpegoptim. Both drop every marker, including the ICC colour profile. A Display P3 photograph from an iPhone then gets interpreted as sRGB, and its colours shift. squint strips location, camera, and timestamp metadata, and keeps the colour profile.

## Roadmap

- **v1** — ImageOptim parity plus the perceptual engine. PNG/JPEG/GIF/SVG, same format, same dimensions, in place. Context menu. ICC preserved, GPS stripped. Nothing else.
- **v1.1** — recipes, and with them the Email/Social presets and dimension caps
- **v1.2** — WebP/AVIF output and SVG rasterization via resvg
- **v1.3** — PDF compression

## Design rules

These hold across every release.

**Never strip the colour profile.** Remove GPS, camera, and timestamp metadata. Keep ICC. Bake orientation into the pixels rather than leaving it as a tag.

**In-place writes require a single same-format output.** A run that produces exactly one file in the same format can overwrite the original. A run that produces two or more outputs, or a different format, must write beside the original and must not modify it.

**Complexity belongs in the preset, not at the point of use.** The Finder context menu offers named destinations. It does not offer settings.

## Engine

| Component | Library | License |
|---|---|---|
| Perceptual metric | [ssimulacra2](https://github.com/rust-av/ssimulacra2) | BSD-2-Clause |
| SVG rasterization (v1.2) | [resvg](https://github.com/linebender/resvg) | Apache-2.0 |
| PNG optimization | [oxipng](https://github.com/shssoichiro/oxipng) | MIT |
| JPEG encoding | [mozjpeg](https://github.com/mozilla/mozjpeg) | BSD-3-Clause |

squint targets a permissively licensed dependency set so that the application can stay under MIT. `dssim` was evaluated for the perceptual metric and rejected: it is AGPL-3.0, and SSIMULACRA2 correlates better with human ratings.

Distribution is a notarized Developer ID build, outside the Mac App Store. Overwriting files in place and running bundled helper executables both require an unsandboxed application.

## License

MIT. See [LICENSE](LICENSE).
