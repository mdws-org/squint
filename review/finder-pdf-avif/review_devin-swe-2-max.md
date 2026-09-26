I'll read all the listed files first.The diff output was truncated at line 331. Let me read the remainder.Let me check whether `core/src/lib.rs` is also present, since question 3 needs it.Read all seven files plus the full diff; `core/src/lib.rs` and `Job.swift` are absent from the lane, so engine internals are taken from the brief's stated contract and `Job.mode` is flagged where it matters.

## 1. Can a PDF be overwritten by a non-PDF, or a picture by a PDF?

**PDF through an in-place preset — confirmed safe by reading.** Trace through `JobQueue.process` (JobQueue.swift:127-184):

- `isIsobmff` (line 138) is false for `%PDF`, so the pre-check passes.
- `Engine.optimize(format: .jpeg)` (lines 142-148) returns PDF bytes; `converted` is false (PDF→PDF is not a conversion per the engine contract; FFI maps `converted_from.is_some()` at ffi.rs:202).
- `outputExtension` returns `"pdf"` (Engine.swift:55), but it is dead weight here: `Preset.destination` (Preset.swift:76) returns `url` unchanged whenever `writesInPlace`.
- `landsOnSource` is true (destination is the same URL object), `converted` is false, so `Writer.replaceInPlace` swaps PDF for PDF. Correct.

**PDF through the AVIF preset — confirmed refused.** `writesInPlace` is false, so the ISOBMFF check is skipped anyway; `optimize_as` returns `Error::ReadOnlyFormat` → `SQUINT_ERR_READ_ONLY` → `Engine.Failure` thrown at JobQueue.swift:142 → `.failed`. Never reaches a write. The FFI test at ffi.rs:297-303 pins this.

**Picture overwritten by a PDF: impossible.** The engine only emits `%PDF` bytes from PDF input, and then only under format JPEG. The one structural caveat: for in-place presets, `landsOnSource` is *always* true (destination is `url` itself), so the sole guard against a hypothetical engine emitting wrong-kind bytes is `converted` (JobQueue.swift:157). Under the stated engine rules (`converted_from` set whenever output kind ≠ input kind), that guard is airtight — a JPEG result for a PDF would be refused at line 157. Confirmed contingent on the engine contract holding.

## 2. `landsOnSource`

`destination.path.lowercased() == url.path.lowercased()` (JobQueue.swift:154). Every asymmetry I could construct **fails toward refusal, never toward overwrite** — confirmed by reading:

- **Case-sensitive volume**: `Photo.AVIF` → destination `Photo.avif` are distinct files, but the lowercased compare yields true → refused with "already an AVIF". A false refusal on a technically-legal conversion; cheap and safe.
- **Unicode**: Swift `String ==` is canonical-equivalence-aware, and `lowercased()` preserves that, so NFC/NFD spellings compare equal — which is *correct* on APFS, since it is normalization-insensitive and both spellings name the same file. A normalization-sensitive exotic volume could only produce a false refusal.
- **Symlinks/aliases**: `destination` is derived from `url.deletingLastPathComponent()`, so it inherits the source's path spelling verbatim; a false negative would require the same file under two spellings, which cannot arise when one path is built from the other.
- **Trailing slash / non-file URL**: a `file:///dir/photo.jpg/` spelling is not a real file; `replaceItemAt` would throw → `.failed`, honest.
- **Percent-encoding**: `.path` is consistently decoded on both sides; literal-`%` names decode to distinct strings. No collision.
- **Hard link**: a pre-existing `photo.avif` that is a hard link to `photo.png`'s inode is *not* caught by `landsOnSource` (different path strings), but the `.atomic` write at line 169 renames a new inode over the directory entry rather than writing through — the source inode is untouched. Safe, but see Q5 for the ordinary collision case.

Verdict: right, and biased the correct direction.

## 3. `converted` vs the guard

FFI sets `converted` iff `converted_from.is_some()` (ffi.rs:202). Per the engine contract: JPEG-format output sets it only for foreign decodes; AVIF-format output always sets it. No input produces `converted = 0` with a different-kind output. The other direction — `converted` set while writing in place — is unreachable: `converted && landsOnSource` is refused first. Consistent.

Two notes:

- `SQUINT_FORMAT_WEBP_LOSSLESS` is exposed in the header/FFI (ffi.rs:15) but `Engine.Format` (Engine.swift:30-33) has no `webp` case, so that arm is dead from Swift — fine, but if ever wired, the same converted semantics hold.
- **Asymmetric hardening**: the unknown-*format* match arm refuses (ffi.rs:155-159), yet the *mode* match still collapses `_ => Mode::Fast` (ffi.rs:168). The comment at ffi.rs:162-164 explains exactly why silent fallback is dangerous — for mode it is still doing it. A corrupted mode code becomes a re-encode. Minor, pre-existing pattern, but the diff half-fixed a defect class.

## 4. `outputExtension`

Reads only `result.data` (Engine.swift:53-58); input bytes never reach it — `destination` is computed from `result.outputExtension` alone (JobQueue.swift:149). Check order PNG → `%PDF` → `ftyp` → `jpg` is sound: PNG's magic is checked first, a PDF can't carry `ftyp` at offset 4 (bytes 4+ are `-1.x`), and an engine-written JPEG starts `FF D8 FF E0/E1`, where offset 4-7 is a big-endian segment length plus `JF`/`Ex` — bytes `66 74 79 70` there would require a 0x6674-length segment starting `yp`, which no JFIF/Exif encoder emits. Worst case if it ever happened: a beside-copy JPEG named `.avif` — cosmetic. The `count >= 12` guard prevents a short-buffer false positive.

## 5. Name collisions — sharper than the email precedent

Same stem, different extensions in **one batch** is a real race: `concurrencyLimit` runs jobs in parallel, so selecting `photo.jpg` + `photo.png` for Convert to AVIF has two tasks racing `result.data.write(to: photo.avif, .atomic)` (JobQueue.swift:169); last rename wins, **both jobs report `.done`**. Suspected by reading — `.atomic` is rename-over with no `O_EXCL` check — not observed.

More important: `photo.avif` is a plausible *unrelated user file* (a download), unlike `photo-email.jpg` which only this app produces. Converting `photo.png` silently clobbers a `photo.avif` the user already had. Trade-off for the owner: numbering (`photo-1.avif`) fixes clobbering but breaks idempotent re-runs — today, re-converting `photo.jpg` cleanly replaces `photo.avif`; numbered output would litter a new file per run. A third option is failing when the destination exists, which also breaks re-runs. The email-preset precedent makes replace-on-collision defensible, but the probability of colliding with an unrelated file is much higher for a bare extension than for `-email`.

## 6. Services list

- **`public.heif` likely re-admits AVIF**: Apple's UTI tree conforms `public.avif` to `public.heif`, so declaring `public.heif` (diff.patch:340) probably makes Finder offer Convert to AVIF for `.avif` files despite the exclusion — consistent with the live-verified result that it ran on an existing `.avif`. Outcome is a safe refusal at JobQueue.swift:157-162; the wart is a menu item that always reports failure. Suspected mechanism.
- **`isSupported` is now over-broad per-entry** (ServiceProvider.swift:114-117): it admits `pdf` into *every* entry's second filter, not just the two in-place ones. A PDF reaching the email/social preset yields a `name-email.pdf` PDF rewrite — unadvertised but benign; via AVIF, a READ_ONLY refusal; via strip, an in-place metadata strip the entry doesn't declare. All honest, but the "list and `NSSendFileTypes` change together" invariant in the comment is now false by construction.
- **Silent semantic loss**: `org.webmproject.webp` includes animated WebP and `public.heif` includes HEIF sequences; conversion presumably yields a still AVIF, silently dropping frames. Suspected — engine decode behavior unverified without lib.rs.
- Mixed-selection filtering: adding `com.adobe.pdf` to the two in-place entries means a PDF+HEIC selection hides them entirely — same conservative Finder behavior as before.
- Stale error string: `"No JPEG or PNG images were selected"` (ServiceProvider.swift:80) was already wrong; worse now that the filter admits eleven extensions.

## 7. Concurrency

`concurrencyLimit` keys on `job.mode` (JobQueue.swift:48) — **not visible whether `Job.mode` returns `preset.mode` or a stored field**; `Job.swift` wasn't provided. If it is `preset.mode`, AVIF jobs correctly bind the batch to ~3 GB/job. If `Job` stores a separate `mode` that defaults to `.fast` for preset-created jobs, an AVIF batch would run at core-count concurrency with quality-mode memory — the exact blowup the limit exists to prevent. **Verify `Job.mode`.** Passes recompute the limit per pass (lines 97-105), so jobs added mid-run are re-bounded correctly. PDF quality jobs are counted as quality like any other; whether one PDF's internal search exceeds 3 GB/job is engine-side and unverifiable here.

## 8. Silent failures

- No path returns success having written nothing: `.done` only follows a write, and every write failure throws into `.failed`. One thin edge: `FileManager.replaceItemAt` is documented to return an optional and its result is discarded (Writer.swift:24) — a nil-without-throw would report `.done` without replacing. Low confidence, Apple documents it as throwing on failure.
- `replaceItemAt` failure leaves `.squint-*.tmp` litter if the best-effort `removeItem` also fails (Writer.swift:26) — error is still reported honestly.
- The Q5 collision is the closest thing to a real silent loss: two `.done` rows, one file.
