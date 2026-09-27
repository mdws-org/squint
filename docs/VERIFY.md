# Verify the claims

Everything the README says Squint removes or keeps can be checked with [ExifTool](https://exiftool.org/) on a file Squint has written. This page shows the commands and what they returned on 2026-09-26 against version 0.8.1, so a reader can repeat them and compare.

```
brew install exiftool
```

The command line binary used below is the engine's own harness, built by `cargo build --release` in `core/`. The Finder entries call the same engine through the same functions, so what the harness writes is what the application writes.

## What Remove Location Data removes, and what it keeps

Six files, one per format, each given the fields a real file carries: camera make and model, the date taken, a GPS position, an XMP creator, a comment where the format has one, and a PDF info dictionary. Then:

```
squint FILE --mode strip --out STRIPPED
exiftool -G1 -s FILE
exiftool -G1 -s STRIPPED
```

The first column of `exiftool -G1` names the group each tag came from. Groups that describe the container itself (`File`, `JFIF`, `PNG`, `RIFF`, `GIF`, `PDF`, `IFD0` for a TIFF's own geometry, and ExifTool's `Composite` and `ExifTool` groups) are computed from the file's structure and stay. Groups that carry metadata go.

| format | bytes before | bytes after | groups before | groups after |
|---|---|---|---|---|
| JPEG | 27,070 | 23,748 | ExifIFD, GPS, IFD0, JFIF, XMP-dc, XMP-x | none but the container |
| TIFF | 924,936 | 924,936 | ExifIFD, GPS, IFD0, XMP-dc, XMP-x | IFD0 (the picture's own geometry) |
| PNG | 2,886 | 1,952 | ExifIFD, GPS, IFD0, PNG, XMP-dc, XMP-x | PNG (the picture's own geometry) |
| WebP | 5,968 | 2,734 | ExifIFD, GPS, IFD0, RIFF, XMP-dc, XMP-x | RIFF (the picture's own geometry) |
| GIF | 89,485 | 89,460 | GIF, with a Comment | GIF, no Comment |
| PDF | 20,138 | 16,273 | PDF, XMP-dc, XMP-pdf, XMP-x | PDF (page count and version) |

A TIFF keeps its byte count because the strip destroys the metadata where it lies rather than cutting it out, which the README explains under Metadata.

Checking the specific fields, which is the shorter test:

```
exiftool -s -s -s -GPSLatitude -Model -DateTimeOriginal -XMP:Creator -Comment -Author STRIPPED
```

returned nothing for every one of the six outputs.

## The colour profile survives every path

The same four raster files, each given a real Display P3 profile (536 bytes, taken from an iPhone photograph), then stripped:

```
exiftool "-ICC_Profile<=display-p3.icc" FILE
squint FILE --mode strip --out STRIPPED
exiftool -s -s -s -ICC_Profile:ProfileDescription FILE STRIPPED
```

| format | profile before | profile after strip | GPS after strip |
|---|---|---|---|
| JPEG | Display P3 | Display P3 | gone |
| TIFF | Display P3 | Display P3 | gone |
| PNG | Display P3 | Display P3 | gone |
| WebP | Display P3 | Display P3 | gone |

And a real photograph (iPhone 17e, Display P3, 2,743,830 bytes) through all three modes:

| mode | bytes | profile | Model | DateTimeOriginal |
|---|---|---|---|---|
| original | 2,743,830 | Display P3 | iPhone 17e | 2026:08:23 11:20:34 |
| strip | 2,739,520 | Display P3 | gone | gone |
| fast | 685,903 | Display P3 | gone | gone |
| quality | 930,340 | Display P3 | gone | gone |

## The pixels are untouched by Strip

```
squint STRIPPED --against FILE
```

scores the two against each other with SSIMULACRA2 and returns 100 when the pixels are identical, which is what Strip must produce for every format it copies through.

## Never grows

Run any mode on a file it cannot improve. The harness reports the refusal and writes nothing; the application shows "already optimal" and leaves the file alone. A conversion that would come out larger reports "would be larger than the original, so none was written".

## What this page does not show

Whether a malformed file can crash the engine. That is what `core/fuzz/` and the malformed-JPEG corpus in `tools/` exist for, and `SECURITY.md` describes both. Nor is any of this an independent audit; it is the author's measurement, written so that it can be repeated.
