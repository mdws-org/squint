//! Development harness for the squint engine.
//!
//! This drives the engine without a user interface so that quality and speed can
//! be measured against real photographs before any application exists.

use squint_core::{search, encode_jpeg, score, extract_icc, extract_orientation, has_gain_map, optimize, png, Hdr, Image, Mode, JPEG_SCORE_CEILING};
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
  --out          write the result to this path
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
            // Negative means lossless, matching the C interface. A value that
            // does not parse is a mistake worth stopping for, not a silent
            // switch to a different kind of compression.
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
    if mode == "strip" {
        let t0 = Instant::now();
        match optimize(&bytes, Mode::Strip, 0.0, 0.0, None) {
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
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let t0 = Instant::now();
        let measure = mode == "quality";
        let effort = if mode == "quality" { png::Effort::Thorough } else { png::Effort::Quick };
        match png::optimize_png(&bytes, png_min_quality, measure, effort) {
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

    let mut image = match Image::decode(&bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    };
    let icc = extract_icc(&bytes);
    let orientation = extract_orientation(&bytes);
    image.apply_orientation(orientation);

    println!(
        "{}  {}x{} = {:.2} MP  {:.0} KB",
        path,
        image.width,
        image.height,
        image.megapixels(),
        bytes.len() as f64 / 1024.0
    );
    println!(
        "         icc {}  orientation {}{}",
        match &icc { Some(p) => format!("{} bytes preserved", p.len()), None => "absent".into() },
        orientation,
        if orientation != 1 { " (baked into pixels)" } else { "" }
    );

    // Scoring one file against another, which is how a fixed-quality result gets
    // a perceptual number attached to it.
    let _ = &icc;
    if let Some(other) = against {
        let ob = std::fs::read(&other).unwrap_or_else(|e| { eprintln!("could not read {other}: {e}"); std::process::exit(1) });
        let oi = Image::decode(&ob).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
        let t0 = Instant::now();
        match score(&image, &oi) {
            Ok(s) => println!("compare {} vs {}  score {:.4}  {:.0} KB -> {:.0} KB  {:.3}s",
                path, other, s, bytes.len() as f64 / 1024.0, ob.len() as f64 / 1024.0, t0.elapsed().as_secs_f64()),
            Err(e) => { eprintln!("{e}"); std::process::exit(1) }
        }
        return;
    }

    let started = Instant::now();
    match mode.as_str() {
        "fast" => {
            let out = encode_jpeg(&image, fixed_quality, icc.as_deref()).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
            let hdr = if has_gain_map(&bytes) { Hdr::Dropped } else { Hdr::Absent };
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "fast     q{:<5.1} {:>7.0} KB  {:>5.1}% of original  {:.3}s  (no metric evaluated){}",
                fixed_quality,
                out.len() as f64 / 1024.0,
                100.0 * out.len() as f64 / bytes.len() as f64,
                elapsed,
                hdr_note(hdr)
            );
            if let Some(o) = &out_path {
                std::fs::write(o, &out).unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                println!("         wrote {o}");
            }
        }
        "quality" => {
            if target > JPEG_SCORE_CEILING {
                eprintln!(
                    "target {target:.0} is above the JPEG ceiling of about {JPEG_SCORE_CEILING:.0}; \
                     the search cannot converge"
                );
                std::process::exit(1)
            }
            match search(&image, target, probes, bytes.len(), icc.as_deref()) {
                Ok(r) => {
                    let elapsed = started.elapsed().as_secs_f64();
                    for p in &r.probes {
                        println!(
                            "  probe   q{:<5.1} score {:>7.3}  {:>7.0} KB{}",
                            p.quality,
                            p.score,
                            p.bytes as f64 / 1024.0,
                            if p.quality == r.chosen.quality { "   <- chosen" } else { "" }
                        );
                    }
                    let out = r.data;
                    let hdr = if has_gain_map(&bytes) { Hdr::Dropped } else { Hdr::Absent };
                    println!(
                        "quality  q{:<5.1} {:>7.0} KB  {:>5.1}% of original  score {:.3}  {} probes  {:.3}s{}",
                        r.chosen.quality,
                        out.len() as f64 / 1024.0,
                        100.0 * out.len() as f64 / bytes.len() as f64,
                        r.chosen.score,
                        r.probes.len(),
                        elapsed,
                        hdr_note(hdr)
                    );
                    if let Some(o) = &out_path {
                        std::fs::write(o, &out).unwrap_or_else(|e| { eprintln!("write failed: {e}"); std::process::exit(1) });
                        println!("         wrote {o}");
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1)
                }
            }
        }
        "score" => {
            let s = score(&image, &image).unwrap();
            println!("self-score {s:.4} (sanity check, expect 100)");
        }
        _ => usage(),
    }
}
