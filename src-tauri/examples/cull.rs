//! Headless smoke test for the analysis pipeline.
//!
//! Runs the real pipeline over a folder and prints what it found, so the
//! scoring can be sanity-checked against actual photographs without launching
//! the UI.
//!
//!   cargo run --release --example cull -- ../test_photos [limit]

use culling_lib::faces::FaceDetector;
use culling_lib::model::{EyeState, ScoreSettings, Verdict};
use culling_lib::pipeline::{self, RunOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let folder = PathBuf::from(args.next().unwrap_or_else(|| "../test_photos".into()));
    let limit: Option<usize> = args.next().and_then(|s| s.parse().ok());

    println!("folder : {}", folder.display());

    let mut models = culling_lib::pipeline::Models::default();
    match FaceDetector::load(None) {
        Ok(d) => {
            println!("faces  : YuNet loaded");
            models.faces = Some(Arc::new(d));
        }
        Err(e) => println!("faces  : DISABLED ({e})"),
    }
    match culling_lib::subjects::SubjectDetector::load(None) {
        Ok(d) => {
            println!("subject: YOLOX loaded");
            models.subjects = Some(Arc::new(d));
        }
        Err(e) => println!("subject: DISABLED ({e})"),
    }

    // When a limit is given, copy that many files into a scratch folder so the
    // pipeline still sees a normal directory.
    let target = match limit {
        Some(n) => {
            let tmp = std::env::temp_dir().join("culling_smoke");
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp)?;
            let mut count = 0;
            for p in culling_lib::scan::scan_folder(&folder, false)? {
                if count >= n {
                    break;
                }
                let dst = tmp.join(p.file_name().unwrap());
                std::fs::copy(&p, &dst)?;
                count += 1;
            }
            println!("limit  : {count} files (copied to {})", tmp.display());
            tmp
        }
        None => folder.clone(),
    };

    let settings = ScoreSettings::default();
    let opts = RunOptions {
        recursive: false,
        detect_faces: models.faces.is_some(),
        detect_subjects: models.subjects.is_some(),
        force_reanalyse: true,
        settings: settings.clone(),
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let d2 = done.clone();

    let items = pipeline::run(&target, &opts, models, cancel, move |p| {
        let n = d2.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 25 == 0 || n == p.total {
            print!("\r  {} / {}   ", n, p.total);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    })?;
    let elapsed = started.elapsed();
    println!("\n");

    let n = items.len();
    if n == 0 {
        println!("no images found");
        return Ok(());
    }

    let keep = items.iter().filter(|a| a.verdict == Verdict::Keep).count();
    let maybe = items.iter().filter(|a| a.verdict == Verdict::Maybe).count();
    let reject = items.iter().filter(|a| a.verdict == Verdict::Reject).count();
    let groups: std::collections::HashSet<u32> = items.iter().filter_map(|a| a.group_id).collect();
    let grouped = items.iter().filter(|a| a.group_id.is_some()).count();
    let with_faces = items.iter().filter(|a| !a.faces.is_empty()).count();
    let total_faces: usize = items.iter().map(|a| a.faces.len()).sum();
    let closed = items
        .iter()
        .filter(|a| a.faces.iter().any(|f| f.eye_state == EyeState::Closed))
        .count();
    let failed = items.iter().filter(|a| a.error.is_some()).count();

    println!("== {n} photos in {:.1}s ({:.0} ms each) ==", elapsed.as_secs_f32(), elapsed.as_millis() as f32 / n as f32);
    println!();
    println!("  keep   {keep:>5}  ({:.0}%)", 100.0 * keep as f32 / n as f32);
    println!("  maybe  {maybe:>5}  ({:.0}%)", 100.0 * maybe as f32 / n as f32);
    println!("  reject {reject:>5}  ({:.0}%)", 100.0 * reject as f32 / n as f32);
    println!();
    println!("  burst groups     {}  covering {grouped} frames", groups.len());
    println!("  frames w/ faces  {with_faces}  ({total_faces} faces total)");
    println!("  frames w/ a closed-eye face  {closed}");
    if failed > 0 {
        println!("  FAILED TO ANALYSE  {failed}");
        for a in items.iter().filter(|a| a.error.is_some()).take(5) {
            println!("    {} — {}", a.meta.file_name, a.error.as_deref().unwrap_or(""));
        }
    }

    // Raw Laplacian variance, so the log bounds in metrics::normalise_sharpness
    // can be checked against real photographs instead of assumed.
    let mut raw: Vec<f32> = items
        .iter()
        .filter(|a| a.error.is_none())
        .map(|a| a.metrics.sharpness_raw)
        .filter(|v| *v > 0.0)
        .collect();
    raw.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !raw.is_empty() {
        println!("\n  raw Laplacian variance (log10) — calibration check");
        for p in [0.02, 0.10, 0.25, 0.50, 0.75, 0.90, 0.98] {
            let v = raw[((raw.len() - 1) as f32 * p) as usize];
            println!(
                "    p{:<3.0} {:>12.6}   log10 {:>7.2}   -> normalised {:.2}",
                p * 100.0,
                v,
                v.log10(),
                culling_lib::metrics::normalise_sharpness(v)
            );
        }
    }

    // Eye-state breakdown, to confirm the heuristic is not over-calling blinks.
    let mut eyes = (0usize, 0usize, 0usize, 0usize);
    for a in &items {
        for f in a.faces.iter().filter(|f| f.area > 0.0008) {
            match f.eye_state {
                EyeState::Open => eyes.0 += 1,
                EyeState::Squint => eyes.1 += 1,
                EyeState::Closed => eyes.2 += 1,
                EyeState::Unknown => eyes.3 += 1,
            }
        }
    }
    let eye_total = eyes.0 + eyes.1 + eyes.2 + eyes.3;
    if eye_total > 0 {
        println!("\n  eye state over {eye_total} significant faces");
        println!("    open {}  squint {}  closed {}  unknown {}", eyes.0, eyes.1, eyes.2, eyes.3);

        // Distribution of the underlying openness score. If a shoot contains no
        // blinks the classifier reports none, which is correct but looks
        // identical to a classifier that never fires — so show the raw range
        // and the lowest-scoring faces to prove the signal is alive.
        let mut opens: Vec<(f32, &str)> = items
            .iter()
            .flat_map(|a| {
                a.faces
                    .iter()
                    .filter(|f| f.area > 0.0008 && f.eye_state != EyeState::Unknown)
                    .map(move |f| (f.eye_open, a.meta.file_name.as_str()))
            })
            .collect();
        opens.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if !opens.is_empty() {
            println!(
                "    openness over {} judged faces: min {:.2}  p25 {:.2}  median {:.2}  max {:.2}",
                opens.len(),
                opens[0].0,
                opens[opens.len() / 4].0,
                opens[opens.len() / 2].0,
                opens[opens.len() - 1].0
            );
            print!("    lowest:");
            for (v, name) in opens.iter().take(5) {
                print!("  {name} {v:.2}");
            }
            println!();
        }
    }

    // Subject size distribution — the signal that actually separates a keeper
    // from a record shot.
    let mut areas: Vec<f32> = items
        .iter()
        .filter(|a| a.error.is_none())
        .map(|a| a.subject_stats.main_area * 100.0)
        .collect();
    areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !areas.is_empty() {
        let none = areas.iter().filter(|v| **v <= 0.0).count();
        println!("\n  largest subject as % of frame  ({none} frames with none)");
        for p in [0.10, 0.25, 0.50, 0.75, 0.90, 0.98, 1.00] {
            let v = areas[(((areas.len() - 1) as f32) * p) as usize];
            println!("    p{:<4.0} {:>7.2}%", p * 100.0, v);
        }
    }

    // The frames with the biggest subjects should be at or near the top. If
    // they are not, something downstream is burying them.
    let mut by_subject: Vec<_> = items.iter().filter(|a| a.error.is_none()).collect();
    by_subject.sort_by(|a, b| {
        b.subject_stats
            .main_area
            .partial_cmp(&a.subject_stats.main_area)
            .unwrap()
    });
    println!("\n  biggest subjects, and where they ranked");
    let ranked: Vec<_> = {
        let mut v: Vec<_> = items.iter().filter(|a| a.error.is_none()).collect();
        v.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        v.iter().map(|a| a.meta.id.clone()).collect()
    };
    for a in by_subject.iter().take(10) {
        let place = ranked.iter().position(|id| *id == a.meta.id).unwrap_or(0) + 1;
        println!(
            "    {:>6.2}%  score {:>5.1}  rank {:>4}/{}  {:<16} {}",
            a.subject_stats.main_area * 100.0,
            a.score,
            place,
            ranked.len(),
            a.meta.file_name,
            a.reasons.iter().map(|r| r.code.as_str()).collect::<Vec<_>>().join(",")
        );
    }

    // Name every frame the classifier called a blink, with the face box, so
    // the calls can be checked against the actual photograph.
    let blinks: Vec<_> = items
        .iter()
        .flat_map(|a| {
            a.faces
                .iter()
                .filter(|f| f.eye_state == EyeState::Closed)
                .map(move |f| (a, f))
        })
        .collect();
    if !blinks.is_empty() {
        println!("\n  closed-eye calls ({})", blinks.len());
        for (a, f) in blinks.iter().take(12) {
            println!(
                "    {:<16} open {:.2}  box x{:.3} y{:.3} w{:.3} h{:.3}  area {:.5}",
                a.meta.file_name, f.eye_open, f.x, f.y, f.w, f.h, f.area
            );
        }
    }

    // Score distribution, to check the scale is actually being used.
    println!("\n  score histogram");
    let mut buckets = [0usize; 10];
    for a in &items {
        buckets[((a.score / 10.0) as usize).min(9)] += 1;
    }
    let peak = buckets.iter().copied().max().unwrap_or(1).max(1);
    for (i, c) in buckets.iter().enumerate() {
        let bar = "#".repeat((c * 34 / peak).max(if *c > 0 { 1 } else { 0 }));
        println!("    {:>3}-{:>3} {:>5} {}", i * 10, i * 10 + 9, c, bar);
    }

    let mut sorted: Vec<_> = items.iter().collect();
    sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    println!("\n  best 8");
    for a in sorted.iter().take(8) {
        println!("    {:>6.1}  {:<16} {}", a.score, a.meta.file_name, summarise(a));
    }
    println!("\n  worst 8");
    for a in sorted.iter().rev().take(8) {
        println!("    {:>6.1}  {:<16} {}", a.score, a.meta.file_name, summarise(a));
    }

    // Show one real burst so grouping can be eyeballed.
    if let Some(gid) = groups.iter().min() {
        let mut members: Vec<_> = items.iter().filter(|a| a.group_id == Some(*gid)).collect();
        members.sort_by_key(|a| a.group_rank);
        println!("\n  example group #{gid} ({} frames)", members.len());
        for a in members.iter().take(10) {
            println!(
                "    {}{:>6.1}  {:<16} {}",
                if a.is_group_best { "★ " } else { "  " },
                a.score,
                a.meta.file_name,
                summarise(a)
            );
        }
    }

    Ok(())
}

fn summarise(a: &culling_lib::model::Analysis) -> String {
    let st = &a.subject_stats;
    let mut bits = vec![format!("subj {:>5.2}%", st.main_area * 100.0)];
    bits.push(format!("sharp {:.2}", a.metrics.sharpness_subject));
    if !a.faces.is_empty() {
        let closed = a
            .faces
            .iter()
            .filter(|f| f.eye_state == EyeState::Closed)
            .count();
        bits.push(format!("{} face(s)", a.faces.len()));
        if closed > 0 {
            bits.push(format!("{closed} closed"));
        }
    }
    let reasons: Vec<&str> = a.reasons.iter().map(|r| r.code.as_str()).collect();
    if !reasons.is_empty() {
        bits.push(reasons.join(","));
    }
    bits.join("  ")
}
