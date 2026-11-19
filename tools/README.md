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

## Corpus tools

Three scripts generate malformed inputs and run them through every engine mode. CI runs them on synthetic seeds; a pre-release run uses real photographs kept outside the repository.

```
python3 tools/corpus-seeds.py seeds
python3 tools/corpus-gen.py seeds/seed.png seeds/seed.jpg seeds/seed.heic -o corpus
python3 tools/corpus-run.py core/target/release/squint corpus -o corpus-report.json
```

`corpus-seeds.py` writes a 2401x1601 picture whose pixel at (x, y) is (x, y, 200) modulo 256 plus a few counts of fixed noise, as PNG, and converts it to JPEG and HEIC with `sips`. The width is odd so a row is not a multiple of any alignment Quartz pads to; the size is above the email preset's 2048 px cap so the resize path runs. If `sips` cannot write HEIC the script exits 3 and says so.

`corpus-gen.py` writes deterministic variants of each seed: truncation at five fractions, segment or chunk lengths off by one and claiming gigabytes, missing markers, corrupted CRCs, bit flips inside coded data, and for HEIC an `iloc` extent pointing 1 MiB past the file and an `ispe` on the primary item (found through `pitm` and `ipma`) declaring 60000x60000, a width of 1, or double the real width. The seeds themselves go in as the `control` family. A variant identical to its seed, or a seed that yields none of the families its kind must have, makes the generator exit 2.

`corpus-run.py` runs every file through `fast`, `quality`, `strip` and `email` (fast with `--max-dimension 2048`) under a watchdog, then judges each run. A run is DANGEROUS when the engine timed out, crashed, reported success and wrote nothing, wrote output from a HEIC that was cut short or whose index points past the file, wrote a file the system decoder cannot open, wrote the wrong size for the mode, wrote a file whose bytes are not the format its name claims, or wrote a picture that no longer resembles the seed. Resemblance is the engine's own `--against` score of the output against the seed PNG, decoded by the `image` crate rather than by the path under test; a black frame scores below -700, a correct output above 65, and the floor is 40. Families that corrupt pixels on purpose, and truncated JPEG or PNG inputs whose partial picture is real, skip the resemblance check. The seeds recorded in the manifest must be the ones present, or the run refuses to start.

Exit codes: 0 clean, 1 dangerous runs, 2 the harness itself failed (no executable binary, missing or changed seeds, no controls, nothing tested).

Verified by substitution: a stand-in engine that writes a black JPEG for every input scores 199 dangerous runs on a 61-file corpus, every control included.

## Audit repro files

`audit-2026-11-06.py` writes one repro file per finding of the 2026-11-06 audit (`docs/audits/2026-11-06.md`) and judges an engine's output on them with tools other than squint: `exiftool -a -G1 -u`, Image I/O through `imageio-probe.swift`, and a byte scan with every Flate stream of a PDF inflated.

```
tools/audit-2026-11-06.py make fx core/target/release/squint
tools/audit-2026-11-06.py judge fx core/target/release/squint
tools/audit-2026-11-06.py measure mem core/target/release/squint
```

`make` needs macOS `sips` and `exiftool`; the engine is used only to write the base WebP. `judge` prints a line per check and exits 1 when any check finds a defect. `measure` reports the peak memory of Fast and Quality at 1.5, 3, 6 and 12 megapixels from `/usr/bin/time -l`, and needs about 3.5 GB free. `core/tests/fixtures/orientation-6.webp` and `orientation-6.png` are copies of two files `make` writes.
