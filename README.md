# Squint

A macOS image optimizer that compresses to a perceptual target instead of a fixed quality number.

Most optimizers ask you to choose a quality setting once and then apply it to every image forever. A flat interface screenshot and a noisy photograph do not tolerate the same setting, so one number wastes bytes on the first and visibly damages the second. Squint measures perceived difference per image and finds the smallest file that stays above the threshold you set.

## Status

Working, unreleased. JPEG and PNG are implemented. There is no signed build and no download yet.

What runs today: a drag and drop window, three Finder Services entries, in-place replacement that preserves Finder tags, and a command line harness for measurement.

What does not exist yet: HEIC, TIFF, WebP, AVIF, GIF and SVG input; Balanced mode; recipes; WebP and AVIF output; PDF; automatic updates.

## Why this exists

[ImageOptim](https://imageoptim.com) has been the reference tool in this space for over a decade, and Squint owes its interaction model to it: drop files on a window, or right-click them in Finder, and they get smaller. That part is worth preserving exactly.

Three things are missing from it. It cannot write WebP or AVIF. It produces exactly one output per input, because its job model tracks a single result per file. And its quality is a fixed number applied uniformly, so the result varies in perceived quality from image to image.

One behaviour is worth correcting rather than copying. With strip-metadata enabled, ImageOptim passes `-copy none` to jpegtran and `--strip-all` to jpegoptim. Both drop every marker, including the ICC colour profile. A Display P3 photograph from an iPhone then gets interpreted as sRGB, and its colours shift.

## Measured against ImageOptim

One 4032x3024 Display P3 photograph, on an Apple M1, with ImageOptim in lossy mode at its author's habitual quality of 74.5.

| | bytes | of original | SSIMULACRA2 | colour profile |
|---|---|---|---|---|
| source | 1,465,453 | | | Display P3 |
| Squint, fast mode | 401,879 | 27.4% | 76.95 | **Display P3** |
| ImageOptim | 400,965 | 27.4% | 76.95 | sRGB |

Squint is not smaller. It compresses to the same size at the same perceived quality, spends 914 bytes carrying the colour profile across, and reports the score rather than leaving you to guess.

One hundred files through fast mode, eight at a time, took 9 seconds on an 8 core machine.

## Install

There is no release yet. Build it:

```
git clone https://github.com/mdws-org/squint.git
cd squint/app
xcodegen generate
xcodebuild -project Squint.xcodeproj -scheme Squint -configuration Release build
```

Requirements: Xcode, [XcodeGen](https://github.com/yonaskolb/XcodeGen), and a Rust toolchain. The Xcode build invokes `cargo` to build the engine.

The application is unsandboxed by design. Replacing arbitrary files in place is not possible under the App Sandbox, which is also why ImageOptim ships unsandboxed.

## Use

Drop images on the window, or right-click them in Finder and choose **Services**, then **Squint (Fast)** or **Squint (Quality)**.

Files are replaced in place. Keep copies until you trust it.

Note that an already-open Get Info window will keep showing camera and location data after a file is processed. Finder caches that panel and does not re-read the file. Close the window and open it again.

## Modes

**Fast** is the default. It encodes once at a fixed quality and evaluates no metric, which matches ImageOptim's speed.

**Quality** searches at full resolution and returns the smallest file that still meets the perceptual target.

**Strip** removes metadata and nothing else. The pixels are copied unchanged, so the result is identical to the input image, and only the container shrinks.

**Balanced** is designed but not implemented. It will search a downscaled proxy and then encode at full resolution.

## Metadata

Fast and Quality remove metadata as a consequence of re-encoding: the file is rebuilt from pixels, so nothing survives that is not deliberately written. Strip removes the same things without re-encoding.

For JPEG, every `APPn` segment is dropped except the ICC colour profile, along with comment segments. That covers EXIF and its embedded thumbnail, XMP, IPTC, Multi Picture Format, HDR gain maps, Apple's rotation block, and C2PA content credentials. None of them are recognised individually. Anything not deliberately kept is removed, which is why provenance formats that did not exist when this was written will also go.

For PNG, only the chunks needed to render are kept: `IHDR`, `PLTE`, `IDAT`, `IEND`, `tRNS`, `iCCP`, `sRGB` and `gAMA`. Dropped chunks include `tEXt`, `iTXt` and `zTXt`, where image generators write prompts and seeds, along with `eXIf`, `tIME` and the `caBX` chunk carrying C2PA.

The colour profile is always kept. It carries no personal information, and discarding it shifts the colours of every photograph taken on a modern phone.

Measured on a 4032x3024 iPhone photograph: 1,465,453 bytes to 1,346,322 in 0.004 seconds, with only the colour profile surviving. Scoring the result against the original returns exactly 100, confirming the pixels are untouched.

Apple writes trailing data past the end-of-image marker, where a second embedded image and further XMP live. Stripping stops at that marker rather than copying to the end of the file. A first implementation did not, and XMP survived.

## Design rules

These hold across every release.

**Never strip the colour profile.** Remove GPS, camera, and timestamp metadata. Keep ICC. Bake orientation into the pixels rather than leaving it as a tag.

**Never grow a file.** A source that is already compressed can require more bytes to match at a high target. When that happens the original is kept.

**In-place writes require a single same-format output.** A run producing two or more outputs, or a different format, must write beside the original and must not modify it.

**Complexity belongs in the preset, not at the point of use.** The Finder menu offers named destinations. It does not offer settings.

**Fail loudly.** Every refusal returns a typed error that explains itself. This domain produces failures that look like success, and a metric that returns a plausible number when it has no valid answer is worse than one that stops.

## Engine

| Component | Library | License |
|---|---|---|
| Perceptual metric | [fast-ssim2](https://github.com/imazen/fast-ssim2) | BSD-2-Clause |
| PNG quantization | [libimagequant](https://github.com/ImageOptim/libimagequant) | GPL-3.0-or-later |
| PNG optimization | [oxipng](https://github.com/shssoichiro/oxipng) | MIT |
| JPEG encoding | [mozjpeg](https://github.com/mozilla/mozjpeg) | BSD-3-Clause |
| SVG rasterization (planned) | [resvg](https://github.com/linebender/resvg) | Apache-2.0 |

Squint is GPL-3.0 because it links libimagequant, which is the only PNG quantizer implementing a quality floor. GPL-3 rather than GPL-2 is required, because resvg is Apache-2.0 and Apache-2.0 is incompatible with GPL-2.

Three metrics were measured on the same 12 megapixel photograph, on an Apple M1, median of five runs:

| metric | time | score | licence |
|---|---|---|---|
| dssim | 0.381 s | uncalibrated | AGPL-3.0 |
| **fast-ssim2** | **0.810 s** | 87.81 | BSD-2-Clause |
| rust-av/ssimulacra2 | 1.606 s | 87.75 | BSD-2-Clause |

fast-ssim2 runs about twice as fast as `rust-av/ssimulacra2` and agrees with it to within 0.07. It is pure Rust with `#![forbid(unsafe_code)]` and dispatches NEON at runtime.

dssim is faster still and remains a candidate for search bracketing, but it is not the primary metric: its `1/SSIM-1` output is uncalibrated and carries no published visually-lossless threshold, so a target expressed in it would mean nothing to a person.

A GPU implementation was evaluated and rejected. Its own documentation disables macOS Metal testing because 12 megapixel images wedge the GPU on unified memory, and reports that the score silently becomes zero when that happens.

## Search

The search opens at a quality predicted from the target, interpolates between the probes bracketing it, and collapses its bracket rather than stopping as soon as a probe lands within tolerance. Stopping early was measured to cost 13 to 45 percent in file size, because the goal is the smallest file above a bar rather than a result near a number.

It returns the best satisfying probe rather than the bracket endpoint. Quality is not monotonic in the encoder setting for synthetic content: text and interface screenshots have been measured to invert by up to 3.5 points.

Perceptual targeting is refused below 113 pixels on the shorter side. SSIMULACRA2 misindexes its internal weight table below that size, and a visibly degraded image can score above 90.

Both sides of a comparison must be interpreted in the same colour space. Comparing a Display P3 reference against an untagged candidate shifts the score by 1.5 to 4.4 points, which exceeds the difference between any two implementations.

## Concurrency

Fast mode is bounded by core count. It peaks at 167 MB per file.

Quality mode is bounded by memory, and exceeding that bound is not merely wasteful but harmful. A 12 megapixel comparison peaks at 2.84 GB. Sixteen files on an 8 core, 8 GB machine took 36 seconds at two concurrent, 60 at four, and 116 at eight, against about 63 seconds run one at a time.

## Roadmap

Formats are tracked separately for reading and writing. Squint should read anything a person is likely to have, because Strip mode is useful on a file it cannot re-encode, while writing a format is a larger commitment.

- **v1** — ImageOptim parity plus the perceptual engine, in place, colour profile preserved, location data dropped. JPEG and PNG read and written. HEIC, TIFF, WebP, AVIF, GIF and SVG remain to be read.
- **v1.1** — recipes, the Email and Social presets, dimension caps, and a Finder Sync extension for the preset submenu
- **v1.2** — WebP and AVIF written, and SVG rasterization
- **v1.3** — PDF, both compression and metadata removal

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
