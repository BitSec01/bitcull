//! Does capture time alone identify burst duplicates?
//!
//! Prints the distribution of inter-frame gaps, then compares time-only
//! grouping against the combined time+appearance rule the app uses, counting
//! how often each would merge two frames that do not actually look alike.
//!
//!   cargo run --release --example gaps -- ../test_photos

use culling_lib::hash::hamming;
use culling_lib::model::{Analysis, ScoreSettings};
use culling_lib::pipeline::{self, RunOptions};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Above this pHash distance two frames are clearly different pictures, so a
/// grouping rule that merges them has made a mistake the user would notice.
const DIFFERENT: u32 = 14;

fn main() -> anyhow::Result<()> {
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "../test_photos".into()),
    );

    // Only the hashes and timestamps matter here, so skip every model.
    let opts = RunOptions {
        recursive: false,
        detect_faces: false,
        detect_subjects: false,
        force_reanalyse: false, // reuse the cache; we only want the metrics
        settings: ScoreSettings::default(),
    };

    let mut items = pipeline::run(
        &folder,
        &opts,
        pipeline::Models::default(),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )?;

    // Capture order, sub-second precision where the camera provides it.
    items.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());

    let n = items.len();
    let with_time = items.iter().filter(|a| a.meta.taken_at.is_some()).count();
    let with_subsec = items.iter().filter(|a| a.meta.sub_sec.is_some()).count();

    println!("frames                {n}");
    println!("with capture time     {with_time}");
    println!("with sub-second time  {with_subsec}");
    if with_subsec == 0 {
        println!("\n  NOTE: no sub-second EXIF, so gaps quantise to whole seconds.");
    }

    // Gap to the previous frame.
    let mut gaps: Vec<(f64, u32)> = Vec::new(); // (gap seconds, phash distance)
    for w in items.windows(2) {
        if w[0].meta.taken_at.is_none() || w[1].meta.taken_at.is_none() {
            continue;
        }
        let g = key(&w[1]) - key(&w[0]);
        let d = hamming(w[0].metrics.phash, w[1].metrics.phash);
        gaps.push((g, d));
    }

    println!("\ngap to previous frame");
    let buckets: [(f64, f64, &str); 8] = [
        (0.0, 0.06, "  <0.06s  (>16 fps)"),
        (0.06, 0.12, "  0.06-0.12s"),
        (0.12, 0.25, "  0.12-0.25s"),
        (0.25, 0.5, "  0.25-0.5s"),
        (0.5, 1.0, "  0.5-1s"),
        (1.0, 2.0, "  1-2s"),
        (2.0, 10.0, "  2-10s"),
        (10.0, f64::MAX, "  >10s"),
    ];
    for (lo, hi, label) in buckets {
        let in_bucket: Vec<_> = gaps.iter().filter(|(g, _)| *g >= lo && *g < hi).collect();
        if in_bucket.is_empty() {
            continue;
        }
        let differ = in_bucket.iter().filter(|(_, d)| *d > DIFFERENT).count();
        println!(
            "{label:<22} {:>5} pairs   {:>5} of them look DIFFERENT ({:.0}%)",
            in_bucket.len(),
            differ,
            100.0 * differ as f32 / in_bucket.len() as f32
        );
    }

    // How each rule performs. A "wrong merge" joins two adjacent frames that
    // do not look alike — exactly the mistake that loses a distinct moment.
    println!("\ngrouping rules (adjacent-pair merges over {} pairs)", gaps.len());
    println!("  {:<34} {:>7} {:>10} {:>14}", "rule", "merges", "wrong", "wrong rate");

    for t in [0.2f64, 0.5, 1.0, 2.0] {
        let merged: Vec<_> = gaps.iter().filter(|(g, _)| *g <= t).collect();
        let wrong = merged.iter().filter(|(_, d)| *d > DIFFERENT).count();
        report(&format!("time only, gap <= {t}s"), merged.len(), wrong);
    }

    let s = ScoreSettings::default();
    let combined: Vec<_> = gaps
        .iter()
        .filter(|(g, d)| *g <= s.burst_gap_secs as f64 && *d <= s.hash_distance + 6)
        .collect();
    let wrong = combined.iter().filter(|(_, d)| *d > DIFFERENT).count();
    report(
        &format!(
            "time <= {}s AND appearance",
            s.burst_gap_secs
        ),
        combined.len(),
        wrong,
    );

    let appearance: Vec<_> = gaps.iter().filter(|(_, d)| *d <= s.hash_distance).collect();
    let wrong = appearance.iter().filter(|(_, d)| *d > DIFFERENT).count();
    report("appearance only", appearance.len(), wrong);

    // Longest run a pure-time rule would swallow, which is the other failure
    // mode: continuous shooting through a passage of play has no gaps at all.
    for t in [0.5f64, 2.0] {
        let mut longest = 1usize;
        let mut run = 1usize;
        for (g, _) in &gaps {
            if *g <= t {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 1;
            }
        }
        println!("\n  longest unbroken run at gap <= {t}s: {longest} frames");
    }

    Ok(())
}

fn report(label: &str, merges: usize, wrong: usize) {
    println!(
        "  {label:<34} {merges:>7} {wrong:>10} {:>13.1}%",
        if merges == 0 {
            0.0
        } else {
            100.0 * wrong as f32 / merges as f32
        }
    );
}

fn key(a: &Analysis) -> f64 {
    a.meta.taken_at.unwrap_or(0) as f64 + a.meta.sub_sec.unwrap_or(0) as f64 / 1000.0
}
