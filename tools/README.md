# Tools

`jpeg-segments.py` lists the marker segments in a JPEG and reports which metadata
strings are present in the bytes.

```
python3 tools/jpeg-segments.py original.jpg processed.jpg
```

Use it to confirm metadata is actually gone. Finder's Get Info panel is cached and
will keep reporting camera and location data after a file is processed, so it
cannot be used to verify this. Reading the bytes can.

It found a real defect during development: a first implementation of Strip mode
copied everything from the start-of-scan marker to the end of the file, and XMP
survived, because Apple writes trailing data past the end-of-image marker.

## Malformed-input corpus

`corpus-gen.py` writes malformed variants of a seed image; `corpus-run.py` puts
them through every mode and reports which ones the system decoder could open
before and cannot open after.

```
python3 tools/corpus-gen.py photo.jpg photo.png -o corpus
python3 tools/corpus-run.py target/release/squint corpus
```

Seeds must be real photographs. The malformations are structural (a segment
length wrong by one byte, a chunk claiming two gigabytes, a truncation partway
through entropy-coded data), so a synthetic image with no metadata exercises far
fewer of them. Generation is deterministic: the same seed produces a
byte-identical corpus, and `manifest.json` records the hash of every file.

The audit of 2026-08-19 ran a corpus like this and found what static review had
labelled unverified: both strippers returned partial output as success when a
walk desynced, and because that output was smaller than the original the
never-grow check passed and it went over the photograph. That corpus was scratch
and no longer exists, so these are committed.

`corpus-run.py` exits non-zero when it finds that case, so it can gate a release.

macOS has no `timeout(1)`. The watchdog is `perl -e 'alarm N; exec @ARGV'`, and a
signalled child arrives in Python as `-14`, not the `142` a shell would print.
Scoring against `142` marks every hang as a clean run.

The decode check converts the whole image rather than reading a property.
`sips -g pixelWidth` parses the header and answers yes for a JPEG truncated to a
tenth of its length. Apple's decoder is tolerant and resyncs where ours cannot,
so a file that fails a full conversion is severely broken, not merely damaged.
