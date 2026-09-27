# Security

## What Squint is exposed to

Every file Squint opens arrives from a right-click in Finder or a drop on its window, and nothing about the file is trusted. The attack surface is the engine's parsers: the JPEG segment walker, the PNG chunk walker, the TIFF directory walker, the ISOBMFF box walker (HEIC and AVIF), the RIFF chunk walker (WebP), the GIF block walker, the PDF object walker (through lopdf), the SVG rasterizer (resvg), and the image decoders behind Fast and Quality. A second surface is the write: Squint replaces files in place, so a defect that produces a plausible but wrong output is written over the original.

The engine's rules, from `README.md`, are the ones a report should measure against:

- A malformed file leaves with a typed error and the original untouched. A stripper that returns what it managed to copy is a defect, because a truncated file is smaller than the original and passes every downstream check.
- Nothing unwinds out of the engine: a panic crossing the C boundary aborts the process. A panic reachable from a file is therefore a denial of service against every other file in the batch, and counts as a security defect.
- A declared image size is checked before anything is allocated, and a decoder allocation ceiling sits behind that.
- A picture cannot make the program read the disk: the SVG rasterizer is told to ignore any file a document names.
- The ICC colour profile is kept on every path; location, camera and timestamp metadata are removed on every path that claims to remove them.

## What Squint does not do

It makes no network connection except the Sparkle update check, which the user enables, and which verifies every update against an EdDSA key compiled into the build before installing it. It reads no file it was not handed. It is unsandboxed by design, because replacing arbitrary files in place is not possible under the App Sandbox.

## Reporting

Report a vulnerability privately through GitHub's private vulnerability reporting on this repository (Security tab, "Report a vulnerability"), or to the contact address on https://thebenmeadows.com. Include the file that triggers it when there is one; a file that makes Squint crash, hang, allocate without bound, or write a wrong output over the original is a valid report even when the file itself is malformed.

Reports are acknowledged within seven days. A fix ships as a release with the report credited unless the reporter asks otherwise.

## What is checked continuously

- `cargo deny` and a lockfile check on every push.
- Fuzzing of every parser with libFuzzer on every change to the engine and weekly for longer (`.github/workflows/fuzz.yml`, targets in `core/fuzz/`).
- The malformed-JPEG corpus in `tools/`, which fuzzes segment lengths and truncation and reports any run where a malformed input produced an output.
- The claims about what is removed and what is kept can be checked by anyone with `exiftool`; `docs/VERIFY.md` shows how and what to expect.

## What has not been done

No independent security audit has been performed. The parsers were written by one person and reviewed by outside models in recorded rounds (`review/`), which is not an audit.
