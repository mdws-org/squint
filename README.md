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

- **v1** — ImageOptim parity plus the perceptual engine. HEIC/PNG/JPEG/GIF/SVG in, same format, same dimensions, in place. Context menu. ICC preserved, GPS stripped. Nothing else.
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
| Perceptual metric | [fast-ssim2](https://github.com/imazen/fast-ssim2) | BSD-2-Clause |
| PNG quantization | [libimagequant](https://github.com/ImageOptim/libimagequant) | GPL-3.0-or-later |
| PNG optimization | [oxipng](https://github.com/shssoichiro/oxipng) | MIT |
| JPEG encoding | [mozjpeg](https://github.com/mozilla/mozjpeg) | BSD-3-Clause |
| SVG rasterization (v1.2) | [resvg](https://github.com/linebender/resvg) | Apache-2.0 |

fast-ssim2 implements SSIMULACRA2 and agrees with the libjxl reference to within 0.04 across a quality sweep, while running about twice as fast as the `rust-av/ssimulacra2` crate on Apple Silicon. It is pure Rust with `#![forbid(unsafe_code)]` and dispatches NEON at runtime.

squint is GPL-3.0 because it links libimagequant, which is the only PNG quantizer implementing a quality floor. GPL-3 rather than GPL-2 is required: resvg is Apache-2.0, which is incompatible with GPL-2.

`dssim` was measured at roughly five times faster than SSIMULACRA2 and remains a candidate for search bracketing. It is not the primary metric, because its `1/SSIM-1` output is uncalibrated and carries no published visually-lossless threshold.

GPU evaluation was rejected. See `docs/` for the record.

## Modes

Every image is judged in one of three modes. The mode belongs to a preset, not to a global setting.

**Fast** is the default. It encodes once at a fixed quality and evaluates no metric, which matches ImageOptim's speed.

**Balanced** searches for a quality target using a downscaled proxy, then encodes at full resolution.

**Quality** searches at full resolution and returns the smallest file that still meets the perceptual target.

## Measured cost

One perceptual comparison of a 12-megapixel photograph takes about 1.6 seconds with `rust-av/ssimulacra2`, about 0.76 seconds with fast-ssim2, and about 0.30 seconds with dssim. Measured on an Apple M1. The metric dominates: encoding and decoding the same image costs about 74 milliseconds.

Perceptual targeting must be refused below 113 pixels on the shorter side. SSIMULACRA2 misindexes its internal weight table below that size and returns scores that are not meaningful.

Both sides of a comparison must be interpreted in the same colour space. Comparing a Display P3 reference against an untagged candidate shifts the score by 1.5 to 4.4 points, which is larger than the difference between any two implementations.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
