# Consensus — PDF and AVIF reach the Finder menu (0.8.0)

Brief: `brief.md`. Probes: `probes.md`. Five seats were asked; four answered.

## Lanes

| Lane | Route | Saw | Bytes | Standing |
|---|---|---|---|---|
| DeepSeek V4.1 Flash | `council-ccx.sh deepseek-flash` | diff, all Swift, `ffi.rs`, `lib.rs` | 9 203 | Best of the round: the only lane to find the SVG defect and the filter drift; one refuted memory claim |
| SWE-2 max | `council-devin.sh swe-2-max` | diff, all Swift, `ffi.rs` (no `lib.rs`, no `Job.swift`) | 9 588 | Two hardenings adopted; three suspicions refuted by measurement or reading; the collision race is the sharpest statement of question 5 |
| GPT-6 Luna | Kagi `gpt-6-luna` (fallback: Codex on ccx hit the free-plan cap) | diff, all Swift, `ffi.rs` | 11 406 | Confirmations only; two "suspect" items refuted |
| Gemini | `council-gemini.sh` (agyx, in the tree) | everything by path | 8 154 | Confirmations only, every trace with line references |
| Codex GPT-5.6 | `council-ccx.sh gpt-5.6-terra` | — | 181 | `usage_limit_reached` (ChatGPT free plan); reseated on Kagi |

## Verified defects (all fixed in the branch; evidence in `probes.md`)

1. **An SVG through Convert to AVIF wrote nothing and reported "already optimal".** Reproduced on the CLI: a 233-byte drawing's AVIF is "only reachable at 4631 bytes"; the engine keeps the original and the application mapped that to the wrong outcome. SVG left the entry; a refused conversion now says "an AVIF of this picture would be larger than the original, so none was written". (DeepSeek.)
2. **The extension filter admitted PDF to every entry**, so a PDF could be stripped through an entry Finder never offers it to, or rewritten as `name-email.pdf` by a caller other than Finder. The filter now asks the preset. Remove Location Data gained `com.adobe.pdf`, since the engine already strips a PDF's info dictionary and XMP. (DeepSeek, SWE-2.)
3. **An unknown mode code collapsed to fast** while the new format arm refused unknown codes. Now refused with `SQUINT_ERR_UNKNOWN_MODE`, with a test. (SWE-2.)
4. The refusal message for a conversion that would land on its source claimed the source "is already an AVIF"; the guard fires on the destination, not the content. Reworded. (DeepSeek.)

## Where the lanes converge (4 of 4)

- A PDF cannot be written over by a picture, nor a picture by a PDF: the in-place path carries PDF bytes only, and the AVIF preset refuses a PDF before any destination exists.
- `landsOnSource`'s case-folding can only turn a miss into a refusal, never a refusal into a write over the source. The one cost is a wrong refusal for `Photo.AVIF` on a case-sensitive volume, accepted.
- `outputExtension` reads output bytes only; no JPEG or PNG result can carry `ftyp` at offset 4.
- The `converted` flag and the guard are consistent with the engine's rules; a converted result never reaches `replaceInPlace`.
- No path reports success without a write.

Convergence here is a shared reading of the same eight-question brief, not independent discovery; the traces agree because the code is small. The value of the round was the two lanes that read past the questions.

## Refuted (recorded so the next round does not re-suspect them)

- PDF fast-mode memory at core-count concurrency: the rewrite decodes one picture at a time; 61 MB resident measured. (DeepSeek.)
- `public.heif` re-admitting AVIF: measured false with `UTType`. (SWE-2.)
- `Job.mode` as a stored field: it is `preset.mode`. (SWE-2.)
- Unicode normalisation splitting `landsOnSource`: both paths derive from one URL. (GPT-6.)
- A byte-identical Strip reported as done: the engine returns `NoSmallerResult` when nothing was removed. (GPT-6.)
- Animated WebP or a HEIF sequence silently losing frames: refused before decode. (SWE-2.)

## Tension for the owner: name collisions on `name.avif`

`photo.jpg` and `photo.png` in one folder both convert to `photo.avif`, and a `photo.avif` the user already had is replaced. Selected together in one batch, the two jobs race the atomic rename and both report done with one file on disk (SWE-2, by reading; not observed). The Email and Social entries have always replaced on collision, and `name-email.jpg` is a name only this application produces; a bare `.avif` is not.

- **Keep replace-on-collision** (DeepSeek's recommendation, and the branch as it stands): re-running the entry on the same file replaces its previous result instead of accreting `photo-2.avif`; the same-stem pair is usually the same picture in two containers.
- **Number on collision** (SWE-2 leaning): protects an unrelated `photo.avif` and the in-batch pair at the cost of idempotent re-runs.
- **Refuse when the destination exists**: protects both cases and also breaks re-runs.

The branch keeps the first. The in-batch race could be closed separately by refusing a second job in one pass whose destination another job in that pass already claimed; not done here because it would change the Email and Social entries too.

## What each lane got wrong

- DeepSeek: the PDF memory claim assumed all pictures decoded at once.
- SWE-2: three suspicions (`public.heif`, `Job.mode`, frame loss) that a file it was not given, or a measurement, settles; the collision race is sound.
- GPT-6: nothing wrong, nothing new; its two "suspect" items were correctly labelled as such.
- Gemini: nothing wrong, nothing new.
