# Squint

A macOS image optimizer that compresses to a perceptual target instead of a fixed quality number.

Most optimizers ask you to choose a quality setting once and then apply it to every image forever. A flat interface screenshot and a noisy photograph do not tolerate the same setting, so one number wastes bytes on the first and visibly damages the second. Squint measures perceived difference per image and finds the smallest file that stays above the threshold you set.

## Status

Working. JPEG and PNG are read and written in place. HEIC, AVIF, WebP and SVG are read and re-encoded beside the original, as a JPEG or an AVIF; GIF and TIFF are read far enough to remove their metadata; a PDF has the pictures inside it re-encoded and its own record of who wrote it removed. Builds on the releases page are signed with a Developer ID and notarized from 0.7.1, so they open like any other application. From 0.4.0 the application can update itself: Squint menu, Check for Updates. It asks once whether to check on a schedule, and every update it installs is verified against a key compiled into the build.

What runs today: a drag and drop window, six Finder Services entries, in-place replacement that preserves Finder tags, and a command line harness for measurement.

What does not exist yet: a GIF and a TIFF can only have their metadata removed; an SVG is rasterized, never optimized as a drawing; WebP is written from the command line only, and only losslessly, since lossy WebP would mean a C dependency; AVIF is written only on macOS; further recipes beyond the email and social presets; inside a PDF, a fax-coded page, a JPEG 2000 image or a CMYK image is left exactly as it was; HDR gain maps through a re-encode, which Strip keeps but Fast and Quality report as removed. Balanced mode is built and parked, for the reason given under Modes.

The claims on this page about what is removed and what is kept can be checked with ExifTool; [docs/VERIFY.md](docs/VERIFY.md) shows the commands and what they returned. [SECURITY.md](SECURITY.md) describes what the engine is exposed to, how it is fuzzed, and how to report a defect privately.

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

Download the `.dmg` from [Releases](https://github.com/mdws-org/squint/releases), or with Homebrew:

```
brew install mdws-org/tap/squint
```

Or build it from source below. Either way it needs an Apple silicon Mac running macOS 14 or later; there is no Intel build.

From 0.7.1 the build is signed with a Developer ID and notarized, so it opens on first launch like any other application. Builds before that were not, and macOS refused them until allowed through System Settings, Privacy and Security, **Open Anyway**; an installed earlier build updates itself to a signed one through Check for Updates.

**Launch it once before looking for it in Finder.** The right-click entries are registered by the application itself, and they do not appear until it has run.

### Building from source

Needs Xcode, [XcodeGen](https://github.com/yonaskolb/XcodeGen), and a Rust toolchain. The Xcode build invokes `cargo` to build the engine.

```
git clone https://github.com/mdws-org/squint.git
cd squint/app
xcodegen generate
xcodebuild -scheme Squint -configuration Release -derivedDataPath build build
cp -R build/Build/Products/Release/Squint.app /Applications
open /Applications/Squint.app
```

Without `-derivedDataPath` the built application lands somewhere under `~/Library/Developer/Xcode/DerivedData` and is easy to lose.

The application is unsandboxed by design. Replacing arbitrary files in place is not possible under the App Sandbox, which is also why ImageOptim is distributed unsandboxed.

## Use

Drop images on the window, or right-click them in Finder and choose **Services**, then one of:

- **Squint: Shrink** does the everyday job. It encodes once at a fixed quality and measures nothing. It also takes a PDF, and rewrites it in place with the pictures inside it re-encoded and any downscaled to 150 dpi.
- **Squint: Shrink for Email** writes `name-email.jpg`, resizing to 2048 pixels on the long edge. It accepts JPEG, PNG, HEIC, AVIF, WebP and SVG, since the copy is a new file and the original's format does not matter. The original is not touched: the cap throws resolution away, and a photograph kept as documentation should not lose it because a copy was being made for an email. Measured on a 4032x3024 photograph, the copy is 129 KB, so about thirty fit under any provider's attachment limit.
- **Squint: Shrink for Social** does the same at 1440 pixels and writes `name-social.jpg`, from the same six formats. Smaller because the destinations are different: Instagram shows a feed picture 1080 pixels wide, X recompresses whatever it is given, and a Nostr client recompresses nothing at all, so what is posted is what everyone downloads. Measured on a 5712x4284 photograph, the copy is 331 KB.
- **Squint: Shrink to a Quality Target** searches for the smallest file that still meets a perceptual score. It takes a PDF as Shrink does, searching each picture inside it to the target.
- **Squint: Convert to AVIF** writes `name.avif` beside the original, at the picture's own size, searched to the quality target. It accepts JPEG, PNG, HEIC and WebP; the original is not touched. An AVIF that would be larger than the original is not written, and the entry says so.
- **Squint: Remove Location Data** takes out where and when a photograph was taken, and what took it, without touching the pixels. On a PDF it drops the document's own record of who wrote it, with what, and when.

**Squint: Remove Location Data** also accepts HEIC, which is what an iPhone camera writes by default, AVIF and WebP, which are what a browser hands you, TIFF, GIF, and PDF. It does not accept SVG, because a drawing carries no metadata block this removes. The two in-place shrinking entries do not accept HEIC because squint writes JPEG, and a JPEG must not overwrite a `.heic`; Shrink for Email, Shrink for Social and Convert to AVIF do accept it, since each writes a new file beside it.

The entries show only when everything selected is a type that entry accepts. Select a folder, or mix a HEIC into a batch for the in-place entries, and Squint is absent from the Services menu with nothing to say why. That is Finder filtering on declared types, not a broken install.

Files are replaced in place. Keep copies until you trust it.

Note that an already-open Get Info window will keep showing camera and location data after a file is processed. Finder caches that panel and does not re-read the file. Close the window and open it again.

## Modes

**Fast** is the default. It encodes once at a fixed quality and evaluates no metric, which matches ImageOptim's speed.

**Quality** searches at full resolution and returns the smallest file that still meets the perceptual target. For JPEG the lever is the encoder's quality setting; for PNG it is the number of colours, since that is what PNG trades away. Neither scale predicts a perceptual score, so both are searched rather than assumed.

PNG has an option JPEG does not: leaving the pixels alone, which is identical to the source and so meets any target by construction. A perceptual target on a PNG is never unreachable, only expensive: where no reduction in colours will meet it, the result is the lossless one. Measured on a 400x300 Display P3 screenshot, a target of 80 returns 55 KB scoring 88.3, smaller than fast mode's 61 KB. A target of 90 finds no reduction that qualifies, and the answer is the 121 KB lossless file.

**An SVG** is rasterized rather than optimized: it has no pixels of its own, so the engine draws it at whatever size was asked for and writes a JPEG. That is a conversion, not an optimization, so the result goes beside the original and never over it — a drawing and a picture of a drawing are not the same file, and the drawing is the one you can still edit. Two things worth knowing. The renderer is told to ignore any file a document names, so an `<image href="file:///...">` fetches nothing: a picture handed to this program must not be able to make it read the disk. And the drawing is composited onto white before the alpha is dropped, because JPEG has none and a transparent background over black looks like a mistake. In Finder, Shrink for Email and Shrink for Social accept an SVG, since both write beside the original; the in-place entries and Remove Location Data do not.

**Strip** removes metadata and nothing else, and is the only mode that reads GIF or TIFF; HEIC is read by every mode. From a GIF it takes out comments and every application block it was not told to keep, which is how XMP and anything a future encoder invents leave without being named. It keeps the frame timing, the animation loop count, any text a viewer draws into the frame, and the colour profile. The pixels are copied unchanged, so the result is identical to the input image, and only the container shrinks. An HDR gain map is kept, because it is part of the picture rather than a record of where it was taken.

What it takes out includes **the date the photograph was taken**. That is deliberate, since when a photograph was taken discloses about as much as where. But it is worth naming, because it is the one field a photograph kept as documentation cannot do without, and it is not recoverable once the file has been overwritten. Strip a copy, not the master.

**Balanced** was built and is not shipped. The design searches a downscaled proxy for the quality setting and then encodes once at full resolution. Measured on 2026-09-13, the proxy's score did not predict the full-resolution score closely enough for the result to be trusted to meet the target, so the mode is parked rather than offered.

## Metadata

Fast and Quality remove metadata as a consequence of re-encoding: the file is rebuilt from pixels, so nothing survives that is not deliberately written. Strip removes the same things without re-encoding.

For JPEG, every `APPn` segment is dropped except the ICC colour profile, along with comment segments. That covers EXIF and its embedded thumbnail, XMP, IPTC, Apple's rotation block, and C2PA content credentials. None of them are recognised individually. Anything not deliberately kept is removed, which is why provenance formats that did not exist when this was written will also go. The exception is the HDR gain map, which is put back afterwards: see below.

For PNG, only the chunks needed to render are kept: `IHDR`, `PLTE`, `IDAT`, `IEND`, `tRNS`, the colour chunks `iCCP`, `sRGB`, `gAMA`, `cHRM` and `cICP`, and the animation chunks `acTL`, `fcTL` and `fdAT`. Dropped chunks include `tEXt`, `iTXt` and `zTXt`, where image generators write prompts and seeds, along with `eXIf`, `tIME` and the `caBX` chunk carrying C2PA. Animation frames are kept because they are picture rather than provenance: dropping them leaves a still image and calls it unchanged.

The colour profile is always kept, on every path. That is worth stating precisely, because it was not true until recently: PNG re-encoding went out through an encoder that writes no colour chunks at all, so a Display P3 screenshot came back untagged and was read as sRGB. The profile is now carried over from the source, whole, after the lossless pass. It carries no personal information, and discarding it shifts the colours of every photograph taken on a modern phone. The same reasoning covers `cHRM`: a file that keeps its gamma and loses its primaries has had its colours altered just as surely.

Both strippers refuse rather than return what they managed to copy. A JPEG segment length wrong by a single byte desynchronises the walk, and returning the bytes gathered up to that point produced a headers-only file, smaller than the original and therefore passing every check downstream, which was then written over the photograph. Apple's decoder resynchronises where this one cannot, so the files that triggered it were files that still opened. A malformed image now leaves with a typed error and its original untouched.

Strip also writes back a 32 byte EXIF block holding the orientation and nothing else. Since it copies the pixels through untouched, that tag is the only thing saying which way up they go, and a portrait photograph stripped without it comes back on its side. Which way up a picture goes identifies nobody; GPS, camera identity and Apple's maker note are still gone. Fast and Quality need no such block, because they turn the pixels themselves.

Measured on a 4032x3024 iPhone photograph: 1,465,453 bytes to 1,442,452, with the colour profile and the gain map surviving and nothing else. Scoring the result against the original returns exactly 100, confirming the pixels are untouched.

A TIFF is a header pointing at a chain of directories, and its pixels sit wherever `StripOffsets` says. Removing a tag has two halves and needs both: the entry has to leave the directory, because a reader given a tag pointing at nothing rejects the whole file, and the bytes it pointed at have to be destroyed, because an unreferenced location is still a location to anyone reading rather than parsing. The directory is rewritten shorter in place, so nothing moves. Measured on a 4032x3024 scan: 1,496 bytes destroyed, the EXIF, GPS and maker-note groups gone, the colour profile and the picture untouched.

One trap is worth naming. A tag's declared size cannot be trusted to stay inside the metadata: on a real file the Exif directory sits at offset 8 and one of its entries reaches past 2606, which is where the first image strip begins. Zeroing on the strength of the declaration destroyed the picture. Every region the picture occupies is now collected before anything is written, and anything overlapping one is left alone.

A HEIC is a tree of boxes rather than a stream of segments, and its EXIF and XMP sit in the same blob as the coded picture, addressed by absolute offsets. Cutting them out would move every byte after them and invalidate every one of those offsets, so their contents are overwritten where they lie instead. The file keeps a few kilobytes of dead space and loses what it was asked to lose. Measured on a 4032x3024 photograph: 3,417 bytes destroyed across two items, the colour profile and HDR headroom intact, and the picture byte-identical everywhere else.

ImageIO looks like it should do this and does not. `CGImageDestinationCopyImageSource`, asked to exclude GPS and XMP, works on a JPEG and on a HEIC returns success having changed nothing — the GPS, EXIF and maker note all survive a copy that reports itself as having excluded them. That is why this is done by hand.

An AVIF is the same tree of boxes with AV1 in the tiles instead of HEVC, so it is stripped the same way: the EXIF and XMP items are overwritten where they lie and nothing moves. A file whose brand names a sequence rather than a still is refused.

A WebP is a RIFF file, a chain of chunks that nothing points into by offset, so a dropped chunk genuinely leaves and the file shrinks. The strip keeps the chunks that carry picture (`VP8 `, `VP8L`, `VP8X`, `ALPH`, `ANIM`, `ANMF`) and the colour profile (`ICCP`), and drops everything else, which is how `EXIF`, `XMP ` and anything a future encoder invents leave without being named. A chunk whose declared length runs past the end of the file is a refusal, not a best effort.

A PDF says who wrote it, with what, and when, twice: in its `/Info` dictionary and again in an XMP packet that `/Metadata` points at. Both go, and the pages are untouched. The same strip runs after a compression pass, so a document that has been shrunk has also lost its author.

Apple writes trailing data past the end-of-image marker, where the gain map and further XMP live. Stripping stops at that marker rather than copying to the end of the file. A first implementation did not, and XMP survived.

## High dynamic range

A photograph from a recent iPhone is two images in one file. The primary is the standard range picture; behind it sits a smaller greyscale gain map saying how far to lift each pixel on a display that can show more. The two are bound together by a Multi Picture Format index. On the sample photograph above the map is 2016x1512 and 96,772 bytes, about a fifteenth of the file.

Losing it is quiet. The file still opens, still looks right on an ordinary display, and looks flat on the display it was taken for. That is the same failure as dropping the colour profile, so it is tracked and reported on every result: the window says `HDR kept` or `HDR removed`, and the command line harness prints the same.

**Strip keeps the map.** It is picture data, not a record of where the picture was taken. Its own EXIF is removed and the parameters describing how to apply it are kept, since without those it is an unreadable grey picture. The result was checked against ImageIO, which reads the map back at full size and reports the headroom, exactly as it does for the untouched original.

**Fast and Quality do not, yet.** The container Squint builds is sound: the same index and the same map, attached to a primary encoded by libjpeg-turbo or to the untouched primary that Strip produces, are read by ImageIO without complaint. Attached to a primary that mozjpeg encoded, macOS will not open the file at all, and `sips` reports nothing either. Substituting the colour profile, the quantization tables, the scan mode and the segment order one at a time changed nothing, which places the trigger in mozjpeg's entropy-coded output. A file that will not open is a worse outcome than one that has lost its extra range, so these modes report the loss instead of causing it silently. Carrying the map through a re-encode needs a different encoder for the primary and is not yet done.

## Design rules

These hold across every release.

**Never strip the colour profile.** Remove GPS, camera, and timestamp metadata. Keep ICC. Where the pixels are re-encoded, turn them the right way up and write no orientation tag; where they are copied through untouched, keep the tag, because it is the only thing saying which way up they go.

**Never change how a picture looks without saying so.** Colour profiles and gain maps both decide appearance rather than describe origin. Where one cannot be carried across, the result says which, rather than leaving it to be noticed on a better display months later.

**Never grow a file.** A source that is already compressed can require more bytes to match at a high target. When that happens the original is kept.

**In-place writes require a single same-format output.** A run producing two or more outputs, or a different format, must write beside the original and must not modify it.

**Complexity belongs in the preset, not at the point of use.** The Finder menu offers named destinations. It does not offer settings.

**Fail loudly.** Every refusal returns a typed error that explains itself. This domain produces failures that look like success, and a metric that returns a plausible number when it has no valid answer is worse than one that stops. Nothing is allowed to unwind out of the engine into the application: a panic crossing that boundary aborts the process, so a defect in one file would take every other file in the batch with it.

**Refuse what cannot be opened safely.** A small file can declare an enormous picture. The declared size is checked before anything is allocated, and a decoder allocation ceiling sits behind that, so a forty kilobyte file claiming sixty thousand pixels square is a typed error rather than fourteen gigabytes of allocation.

## Engine

| Component | Library | License |
|---|---|---|
| Perceptual metric | [fast-ssim2](https://github.com/imazen/fast-ssim2) | BSD-2-Clause |
| PNG quantization | [libimagequant](https://github.com/ImageOptim/libimagequant) | GPL-3.0-or-later |
| PNG optimization | [oxipng](https://github.com/shssoichiro/oxipng) | MIT |
| JPEG encoding | [mozjpeg](https://github.com/mozilla/mozjpeg) | BSD-3-Clause |
| JPEG, PNG and WebP decoding; lossless WebP encoding | [image](https://github.com/image-rs/image) | MIT OR Apache-2.0 |
| SVG rasterization | [resvg](https://github.com/linebender/resvg) | Apache-2.0 OR MIT |
| PDF object model | [lopdf](https://github.com/J-F-Liu/lopdf) | MIT |
| HEIC and AVIF decoding, AVIF encoding | Image I/O | Apple system framework, macOS only |

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

## Command line

The engine ships with a harness, built by `cargo build --release` in `core/` and left at `core/target/release/squint`. It exists for measurement: every number on this page came from it, and the Finder entries call the same functions.

```
squint <image> [--mode fast|quality|strip] [--target <score>] [--quality <n>] [--probes <n>]

  fast     encode once at a fixed quality, measure nothing (the default)
  quality  search for the smallest file scoring at or above the target
  strip    remove metadata, leaving the pixels exactly as they were

  --target         perceptual target, 70 general web, 80 high, 90 visually lossless (default 80)
  --quality        fixed quality for fast mode (default 75)
  --probes         maximum encodes during a search (default 6)
  --png-quality    palette quality floor for PNG, negative for lossless (default 70)
  --max-dimension  cap the long edge in pixels; never enlarges (default none)
  --out            write the result to this path
  --format         jpeg (default), avif, or webp; avif and webp are conversions, written beside the original
  --against        score this image against another instead of encoding
```

It reads every format the application reads, including a PDF, and reports what it wrote, the score where one was measured, and whether the colour profile and the HDR gain map survived. Without `--out` it writes nothing and reports what it would have done, which is how a result is measured before a file is touched.

## Shipped

Formats are tracked separately for reading and writing. Squint reads anything a person is likely to have, because Strip is useful on a file it cannot re-encode, while writing a format is a larger commitment.

- **0.1** — the perceptual engine, JPEG and PNG read and written in place, the colour profile kept, the Finder entries, and Strip.
- **0.2** — Shrink for Email, the first preset written beside the original with a size cap. Remove Location Data reads HEIC and TIFF.
- **0.4** — Shrink for Social. The application updates itself through Sparkle. GIF is read far enough to strip it; SVG is read and rasterized beside the original.
- **0.5** — WebP and AVIF are read, for stripping and for re-encoding beside the original.
- **0.6** — AVIF and lossless WebP are written, named on the command line with `--format`. AVIF goes through the system encoder because no pure-Rust one can embed a colour profile, so it is macOS only; WebP is lossless, because the pure-Rust encoder has no other kind, and suits what a PNG suits rather than photographs.
- **0.7** — PDF, both compression and metadata removal. The pictures inside a document are lifted out, re-encoded through the same perceptual search a standalone JPEG gets, and put back; the text, the vectors and the structure are untouched, so the document stays selectable and searchable. Resolution is capped at 150 dpi, which is where nearly all of the saving is: a 14 MB thirty-page scan comes out at 3.0 MB and an 11 MB sheet holding one uncompressed image at 183 KB. From 0.7.1 the build is signed and notarized.
- **0.8** — PDF and AVIF reach the Finder menu: Shrink and Shrink to a Quality Target take a PDF, Convert to AVIF is an entry, and Remove Location Data takes a PDF.

## Open

- Lossy WebP, which would mean a C dependency.
- AVIF written anywhere but macOS, which would mean a pure-Rust encoder that can embed a colour profile.
- The HDR gain map carried through a re-encode. The container is sound; a primary encoded by mozjpeg with the map attached will not open, so this needs a different encoder for the primary.
- Inside a PDF, a fax-coded page, a JPEG 2000 image and a CMYK image are left as they were. The 150 dpi cap is not a control.
- Balanced mode, parked for the reason under Modes.
- A Finder Sync extension, so the presets are a submenu rather than six entries under Services.
- Further recipes beyond the Email and Social presets.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
