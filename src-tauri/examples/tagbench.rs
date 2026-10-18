//! Times tag reading per format: cargo run --release --example tagbench -- <dir> [max per ext]
use std::collections::HashMap;
use std::time::{Duration, Instant};

use lofty::prelude::*;

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let max: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    let mut per: HashMap<String, (usize, Duration, Duration)> = HashMap::new();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = e.path();
        let Some(ext) = p.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()) else { continue };
        if !["mp3", "flac", "m4a", "wav", "ogg"].contains(&ext.as_str()) { continue; }
        let entry = per.entry(ext).or_default();
        if entry.0 >= max { continue; }
        let t = Instant::now();
        let r = lofty::read_from_path(p);
        let full = t.elapsed();
        let t = Instant::now();
        let opts = lofty::config::ParseOptions::new().read_properties(false).read_cover_art(false);
        let _ = lofty::probe::Probe::open(p).and_then(|pr| pr.options(opts).read());
        let light = t.elapsed();
        if let Ok(f) = r { let _ = f.properties().duration(); }
        entry.0 += 1; entry.1 += full; entry.2 += light;
    }
    for (ext, (n, full, light)) in per {
        println!("{ext:5} n={n:3}  full avg {:7.1} ms   no-props/no-art avg {:7.1} ms", full.as_secs_f64() * 1000.0 / n as f64, light.as_secs_f64() * 1000.0 / n as f64);
    }
}
