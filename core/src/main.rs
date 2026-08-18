//! Development harness for the squint engine.
//!
//! This drives the engine without a user interface so that quality and speed can
//! be measured against real photographs before any application exists.

use squint_core::{search, encode_jpeg, score, Image, JPEG_SCORE_CEILING};
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "usage: squint <image> [--mode fast|quality] [--target <score>] [--quality <n>] [--probes <n>]

  fast     encode once at a fixed quality, measure nothing (the default)
  quality  search for the smallest file scoring at or above the target

  --target   perceptual target, 70 general web, 80 high, 90 visually lossless (default 80)
  --quality  fixed quality for fast mode (default 75)
  --probes   maximum encodes during a search (default 6)"
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

    let mut i = 1;
    while i < args.len() {
        let next = args.get(i + 1);
        match args[i].as_str() {
            "--mode" => mode = next.unwrap_or_else(|| usage()).clone(),
            "--target" => target = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--quality" => fixed_quality = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--probes" => probes = next.and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--against" => against = Some(next.unwrap_or_else(|| usage()).clone()),
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
    let image = match Image::decode(&bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    };

    println!(
        "{}  {}x{} = {:.2} MP  {:.0} KB",
        path,
        image.width,
        image.height,
        image.megapixels(),
        bytes.len() as f64 / 1024.0
    );

    // Scoring one file against another, which is how a fixed-quality result gets
    // a perceptual number attached to it.
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
            let out = encode_jpeg(&image, fixed_quality).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "fast     q{:<5.1} {:>7.0} KB  {:>5.1}% of original  {:.3}s  (no metric evaluated)",
                fixed_quality,
                out.len() as f64 / 1024.0,
                100.0 * out.len() as f64 / bytes.len() as f64,
                elapsed
            );
        }
        "quality" => {
            if target > JPEG_SCORE_CEILING {
                eprintln!(
                    "target {target:.0} is above the JPEG ceiling of about {JPEG_SCORE_CEILING:.0}; \
                     the search cannot converge"
                );
                std::process::exit(1)
            }
            match search(&image, target, probes, bytes.len()) {
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
                    println!(
                        "quality  q{:<5.1} {:>7.0} KB  {:>5.1}% of original  score {:.3}  {} probes  {:.3}s",
                        r.chosen.quality,
                        r.chosen.bytes as f64 / 1024.0,
                        100.0 * r.chosen.bytes as f64 / bytes.len() as f64,
                        r.chosen.score,
                        r.probes.len(),
                        elapsed
                    );
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
