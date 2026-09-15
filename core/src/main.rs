//! Development harness for the squint engine.
//!
//! This drives the engine without a user interface so that quality and speed can
//! be measured against real photographs before any application exists.

use squint_core::{score, optimize, optimize_as, png, Hdr, Mode, OutputFormat, Source, JPEG_SCORE_CEILING};
use std::time::Instant;

/// How the gain map fared, for the line the harness prints.
fn hdr_note(hdr: Hdr) -> &'static str {
    match hdr {
        Hdr::Absent => "",
        Hdr::Preserved => "  hdr gain map preserved",
        Hdr::Dropped => "  HDR GAIN MAP DROPPED",
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: squint <image> [--mode fast|quality] [--target <score>] [--quality <n>] [--probes <n>]

  fast     encode once at a fixed quality, measure nothing (the default)
  quality  search for the smallest file scoring at or above the target

  strip    remove metadata, leaving the pixels exactly as they were

  --target       perceptual target, 70 general web, 80 high, 90 visually lossless (default 80)
  --quality      fixed quality for fast mode (default 75)
  --probes       maximum encodes during a search (default 6)
  --png-quality  palette quality floor for PNG, negative for lossless (default 70)
  --max-dimension  cap the long edge in pixels; never enlarges (default none)
  --out          write the result to this path
  --format       jpeg (default), avif, or webp. avif and webp are conversions:
                 the result is a different kind of file and goes beside the
                 original, never over it. webp is lossless and suits what a PNG
                 suits; on a photograph it will be refused for growing the file
  --against      score this image against another instead of encoding"
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let path = &args[0];
    let mut mode = "fast".to_string();
    let mut target = 80.0f64;
    let mut fixed_quality = 75.0f32;
    let mut probes = 6usize;
    let mut against: Option<String> = None;
    let mut out_path: Option<String> = None;
    let mut format = OutputFormat::Jpeg;
    let mut max_dimension: Option<u32> = None;
    // The same default the application sends. It used to be lossless here and
    // quantized there, so every PNG number ever measured on this harness
    // described something the application does not do.
    let mut png_min_quality: Option<u8> = Some(70);

    let mut i = 1;
    while i < args.len() {
        let next = args.get(i + 1);
        match args[i].as_str() {
            "--mode" => mode = next.unwrap_or_else(|| usage()).clone(),
            "--target" => target = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--quality" => fixed_quality = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--probes" => probes = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--against" => against = Some(next.unwrap_or_else(|| usage()).clone()),
            "--out" => out_path = Some(next.unwrap_or_else(|| usage()).clone()),
            "--format" => {
                format = OutputFormat::parse(next.unwrap_or_else(|| usage())).unwrap_or_else(|| usage())
            }
            // Negative means lossless, matching the C interface. A value that
            // does not parse is a mistake worth stopping for, not a silent
            // switch to a different kind of compression.
            "--max-dimension" => {
                let v: u32 = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                max_dimension = (v > 0).then_some(v);
            }
            "--png-quality" => {
                let v: i32 = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                png_min_quality = (v >= 0).then(|| v.min(100) as u8);
            }
            _ => usage(),
        }
        i += 2;
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            std::process::exit(1)
        }
    };
    // Scoring one file against another, which is how a fixed-quality result
    // gets a perceptual number attached to it. Handled before any mode is
    // dispatched, so that a PNG can be the reference: the PNG branch below
    // returns without looking at this flag.
    if let Some(other) = &against {
        let ob = std::fs::read(other).unwrap_or_else(|e| { eprintln!("could not read {other}: {e}"); std::process::exit(1) });
        let reference = Source::open(&bytes, None).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) }).image;
        let candidate = Source::open(&ob, None).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) }).image;
        let t0 = Instant::now();
        match score(&reference, &candidate) {
            Ok(s) => println!("compare {} vs {}  score {:.4}  {:.0} KB -> {:.0} KB  {:.3}s",
                path, other, s, bytes.len() as f64 / 1024.0, ob.len() as f64 / 1024.0, t0.elapsed().as_secs_f64()),
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return;
    }
    if mode == "strip" {
        let t0 = Instant::now();
        match optimize(&bytes, Mode::Strip, 0.0, 0.0, None, probes, None) {
            Ok(r) => {
                println!(
                    "{}  {:>7.0} KB -> {:>7.0} KB  {:>5.1}%  metadata removed, pixels untouched{}  {:.3}s",
                    path,
                    bytes.len() as f64 / 1024.0,
                    r.data.len() as f64 / 1024.0,
                    100.0 * r.data.len() as f64 / bytes.len() as f64,
                    hdr_note(r.hdr),
                    t0.elapsed().as_secs_f64()
                );
                if let Some(o) = &out_path {
                    std::fs::write(o, &r.data).unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                    println!("         wrote {o}");
                }
            }
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return;
    }

    // PNG takes a different path: palette quantization rather than a quality dial,
    // and an alpha channel the metric cannot see directly.
    //
    // Only when a PNG is what was asked for. This branch writes PNG bytes to
    // whatever `--out` names, so reaching it with another format requested
    // produces a file whose contents do not match its name — a worse outcome
    // than refusing, because nothing reports it.
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) && format == OutputFormat::Jpeg {
        let t0 = Instant::now();
        let measure = mode == "quality";
        let effort = if mode == "quality" { png::Effort::Thorough } else { png::Effort::Quick };
        match png::optimize_png(&bytes, png_min_quality, measure.then_some(target), effort, max_dimension) {
            Ok(r) => {
                println!(
                    "{}  {:>7.0} KB -> {:>7.0} KB  {:>5.1}%  {}{}  {:.3}s",
                    path,
                    bytes.len() as f64 / 1024.0,
                    r.data.len() as f64 / 1024.0,
                    100.0 * r.data.len() as f64 / bytes.len() as f64,
                    if r.quantized { "quantized" } else { "lossless" },
                    match r.score { Some(s) => format!("  score {s:.3}"), None => String::new() },
                    t0.elapsed().as_secs_f64()
                );
                if let Some(o) = &out_path {
                    std::fs::write(o, &r.data).unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                    println!("         wrote {o}");
                }
            }
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return;
    }

    // Reported before decoding, because neither can be decoded here at all and
    // the reason should be the one the engine gives rather than a decoder's
    // complaint about a format it was never taught.
    if squint_core::tiff::is_tiff(&bytes) || squint_core::gif::is_gif(&bytes) {
        match optimize(&bytes, Mode::Fast, target, fixed_quality, png_min_quality, probes, None) {
            Ok(_) => unreachable!("neither a TIFF nor a GIF can be re-encoded"),
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
    }

    // A PDF is not a picture, so it never reaches `Source::open`. It goes
    // through `optimize` like everything else rather than being rewritten here:
    // a private copy of the work in the harness is how this tool once came to
    // measure something the application does not do.
    if squint_core::pdf::is_pdf(&bytes) {
        let requested = if mode == "quality" { Mode::Quality } else { Mode::Fast };
        let pages = squint_core::pdf::page_count(&bytes);
        let t0 = Instant::now();
        match optimize(&bytes, requested, target, fixed_quality, png_min_quality, probes, None) {
            Ok(r) => {
                println!(
                    "{}  {} {:>7.0} KB -> {:>7.0} KB  {:>5.1}%  images re-encoded in place  {:.3}s",
                    path,
                    match pages {
                        Some(n) => format!("{n} pages "),
                        None => String::new(),
                    },
                    bytes.len() as f64 / 1024.0,
                    r.data.len() as f64 / 1024.0,
                    100.0 * r.data.len() as f64 / bytes.len() as f64,
                    t0.elapsed().as_secs_f64()
                );
                if let Some(o) = &out_path {
                    std::fs::write(o, &r.data)
                        .unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                    println!("         wrote {o}");
                }
            }
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return;
    }

    let src = match Source::open(&bytes, None) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    };
    let image = src.image;

    println!(
        "{}  {}x{} = {:.2} MP  {:.0} KB{}",
        path,
        image.width,
        image.height,
        image.megapixels(),
        bytes.len() as f64 / 1024.0,
        match src.converted_from {
            Some(f) => format!("  converted from {f}"),
            None => "".into(),
        }
    );
    println!(
        "         icc {}  orientation {}{}",
        match &src.icc { Some(p) => format!("{} bytes preserved", p.len()), None => "absent".into() },
        src.orientation,
        if src.orientation != 1 { " (baked into pixels)" } else { "" }
    );

    let started = Instant::now();
    match mode.as_str() {
        // Both encoding modes run through `optimize`, the same call the
        // application makes. Running a private copy of the work here is how the
        // harness came to measure something the application does not do: its
        // fast path had no never-grow check, so it would happily write a file
        // larger than its input where the application refused.
        "fast" | "quality" => {
            let requested = if mode == "quality" { Mode::Quality } else { Mode::Fast };
            if requested == Mode::Quality && format == OutputFormat::Jpeg && target > JPEG_SCORE_CEILING {
                eprintln!(
                    "target {target:.0} is above the JPEG ceiling of about {JPEG_SCORE_CEILING:.0}; \
                     the search cannot converge"
                );
                std::process::exit(1)
            }
            let r = optimize_as(&bytes, format, requested, target, fixed_quality, png_min_quality, probes, max_dimension)
                .unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
            let elapsed = started.elapsed().as_secs_f64();

            for p in &r.probes {
                println!(
                    "  probe   q{:<5.1} score {:>7.3}  {:>7.0} KB{}",
                    p.quality,
                    p.score,
                    p.bytes as f64 / 1024.0,
                    if Some(p.score) == r.score { "   <- chosen" } else { "" }
                );
            }
            // A percentage of the source says nothing once the format has
            // changed: a vector drawing is a few hundred bytes and any raster
            // of it is thousands, which prints as "3466% of original" and
            // invites a reader to think something went wrong.
            let against_source = match r.converted_from {
                Some(_) => String::new(),
                None => format!("  {:>5.1}% of original", 100.0 * r.data.len() as f64 / bytes.len() as f64),
            };
            println!(
                "{:<8} q{:<5.1} {:>7.0} KB{}{}  {} probes  {:.3}s{}{}",
                mode,
                r.probes.last().map_or(fixed_quality, |p| p.quality),
                r.data.len() as f64 / 1024.0,
                against_source,
                match r.score { Some(s) => format!("  score {s:.3}"), None => "  (no metric evaluated)".into() },
                r.probes.len(),
                elapsed,
                hdr_note(r.hdr),
                match r.converted_from {
                    Some(f) => format!("  written as {} (from {f})", format.extension().to_uppercase()),
                    None => "".into(),
                }
            );
            if let Some(o) = &out_path {
                // A converted result is a different kind of file from its
                // source, so writing it back over that source destroys the
                // original and leaves a name that lies about its contents.
                // The application refuses this from the bytes; the harness has
                // to refuse it too, since it writes wherever it is pointed.
                if r.converted_from.is_some() && std::fs::canonicalize(o).ok() == std::fs::canonicalize(path).ok() {
                    eprintln!(
                        "{} became {}, which must not be written over the original; choose another --out path",
                        r.converted_from.unwrap_or("this file"),
                        format.extension().to_uppercase()
                    );
                    std::process::exit(1)
                }
                std::fs::write(o, &r.data).unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                println!("         wrote {o}");
            }
        }
        "score" => {
            let s = score(&image, &image).unwrap();
            println!("self-score {s:.4} (sanity check, expect 100)");
        }
        _ => usage(),
    }
}
