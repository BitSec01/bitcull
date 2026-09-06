//! The analysis pipeline.
//!
//! One decode per photo feeds every measurement. Decoding is by far the most
//! expensive step, so the working image is computed once at ~2048 px and then
//! reused for sharpness, exposure, hashing, face detection and the thumbnail.

use crate::cache::Cache;
use crate::decode;
use crate::faces::FaceDetector;
use crate::hash;
use crate::metrics;
use crate::model::{Analysis, Metrics, PhotoMeta, Progress, ScoreSettings, SubjectStats, Verdict};
use crate::subjects::SubjectDetector;
use crate::scan;
use anyhow::Result;
use parking_lot::Mutex;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Long edge used for analysis.
///
/// Measured on a real 872-frame shoot: 2048 px cost ~30 ms per frame but was
/// too small on two counts. Focus differences on a 45 MP file largely vanish by
/// then, and a face has to fill 3% of the frame before its eyes span enough
/// pixels to judge — so most eye verdicts came back "unknown".
///
/// 3072 px is 3/8 of a 8192 px Canon frame, so `jpeg-decoder` can still reach
/// it with a clean DCT scale, and it roughly halves both problems for well
/// under double the cost. There was ample headroom.
pub const WORK_SIZE: u32 = 3072;
/// Long edge of the cached grid thumbnail.
pub const THUMB_SIZE: u32 = 640;

pub struct RunOptions {
    pub recursive: bool,
    pub detect_faces: bool,
    pub detect_subjects: bool,
    pub force_reanalyse: bool,
    pub settings: ScoreSettings,
}

/// The models a run may use. Loaded once and shared across worker threads.
#[derive(Default, Clone)]
pub struct Models {
    pub faces: Option<Arc<FaceDetector>>,
    pub subjects: Option<Arc<SubjectDetector>>,
}

/// Analyse every image in `folder`.
///
/// `on_progress` is called from worker threads, so it must be cheap and
/// thread-safe. `cancel` is polled between photos.
pub fn run<F>(
    folder: &Path,
    opts: &RunOptions,
    models: Models,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<Vec<Analysis>>
where
    F: Fn(Progress) + Send + Sync,
{
    let paths = scan::scan_folder(folder, opts.recursive)?;
    let total = paths.len();
    let started = Instant::now();

    let cache = Arc::new(Mutex::new(Cache::open(folder)?));
    if opts.force_reanalyse {
        cache.lock().clear();
    }

    let done = AtomicUsize::new(0);

    let mut results: Vec<Analysis> = paths
        .par_iter()
        .map(|path| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }

            let meta = match scan::read_meta(path) {
                Ok(m) => m,
                Err(e) => {
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    on_progress(progress(n, total, path, "failed", started));
                    return Some(failed_analysis(path, e.to_string()));
                }
            };

            // Reuse a cached result when the file is untouched.
            if let Some(hit) = cache.lock().get(&meta.id, meta.mtime, meta.bytes) {
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                on_progress(progress(n, total, path, "cached", started));
                return Some(hit);
            }

            let analysis = analyse_one(path, meta, &models, opts, &cache);

            if let Some(ref a) = analysis {
                cache.lock().put(a);
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            on_progress(progress(n, total, path, "analysed", started));
            analysis
        })
        .flatten()
        .collect();

    // Restore capture/filename order; rayon returns in completion order.
    results.sort_by(|a, b| a.meta.path.cmp(&b.meta.path));

    if let Err(e) = cache.lock().save() {
        eprintln!("culling: failed to write cache: {e}");
    }

    crate::score::rescore_all(&mut results, &opts.settings);
    Ok(results)
}

fn progress(done: usize, total: usize, path: &Path, stage: &str, started: Instant) -> Progress {
    let elapsed = started.elapsed().as_millis() as u64;
    // Only estimate once there is enough signal for the number to be useful.
    let eta = if done >= 8 && done < total {
        let per = elapsed as f64 / done as f64;
        Some((per * (total - done) as f64) as u64)
    } else {
        None
    };
    Progress {
        done,
        total,
        current: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        stage: stage.to_string(),
        elapsed_ms: elapsed,
        eta_ms: eta,
    }
}

fn analyse_one(
    path: &Path,
    meta: PhotoMeta,
    models: &Models,
    opts: &RunOptions,
    cache: &Arc<Mutex<Cache>>,
) -> Option<Analysis> {
    let rgb = match decode::decode_scaled(path, WORK_SIZE) {
        Ok(img) => img,
        Err(e) => return Some(failed_analysis(path, e.to_string())),
    };
    let rgb = decode::apply_orientation(rgb, meta.orientation);
    let gray = decode::to_gray(&rgb);

    // Write the grid thumbnail from the image we already have in memory.
    let thumb = cache.lock().thumb_path(&meta.id, meta.mtime, THUMB_SIZE);
    if !thumb.exists() {
        let long = rgb.w.max(rgb.h).max(1);
        let scale = (THUMB_SIZE as f32 / long as f32).min(1.0);
        let tw = ((rgb.w as f32 * scale).round() as u32).max(1);
        let th = ((rgb.h as f32 * scale).round() as u32).max(1);
        let small = decode::resize_rgb(&rgb, tw, th);
        if let Ok(bytes) = decode::encode_jpeg(&small, 82) {
            let _ = std::fs::write(&thumb, bytes);
        }
    }

    let smap = metrics::sharpness_map(&gray);
    let (motion, angle) = metrics::motion_blur(&gray);

    let metrics_out = Metrics {
        sharpness_raw: smap.peak,
        sharpness_global: metrics::normalise_sharpness(smap.global),
        sharpness_peak: metrics::normalise_sharpness(smap.peak),
        sharpness_subject: 0.0, // filled in below when a face is found
        focus_x: smap.focus_x,
        focus_y: smap.focus_y,
        motion_blur: motion,
        motion_angle: angle,
        exposure: metrics::exposure(&gray),
        noise: metrics::noise(&gray),
        phash: hash::phash(&gray),
        dhash: hash::dhash(&gray),
        signature: hash::signature(&gray),
    };

    let faces = if opts.detect_faces {
        match models.faces.as_deref() {
            Some(d) => d.detect(&rgb, &gray).unwrap_or_else(|e| {
                eprintln!("culling: face detection failed for {}: {e}", path.display());
                Vec::new()
            }),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut subjects = if opts.detect_subjects {
        match models.subjects.as_deref() {
            Some(d) => d
                .detect(&rgb, &gray, opts.settings.subject_confidence)
                .unwrap_or_else(|e| {
                    eprintln!(
                        "culling: subject detection failed for {}: {e}",
                        path.display()
                    );
                    Vec::new()
                }),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let subject_stats = crate::subjects::summarise(
        &mut subjects,
        &opts.settings.subject_classes,
        metrics_out.sharpness_peak,
        Some(&smap),
    );

    let mut metrics_out = metrics_out;
    metrics_out.sharpness_subject = faces
        .iter()
        .filter(|f| f.area > 0.0008)
        .map(|f| f.sharpness)
        .chain(subjects.iter().filter(|s| s.counts).map(|s| s.sharpness))
        .fold(metrics_out.sharpness_peak, f32::max);

    Some(Analysis {
        meta,
        metrics: metrics_out,
        faces,
        subjects,
        subject_stats,
        score: 0.0,
        verdict: Verdict::Maybe,
        reasons: Vec::new(),
        group_id: None,
        is_group_best: false,
        group_rank: 0,
        user_pick: None,
        rating: 0,
        error: None,
    })
}

/// A placeholder record so unreadable files still appear in the grid rather
/// than vanishing without explanation.
fn failed_analysis(path: &Path, err: String) -> Analysis {
    let meta = scan::read_meta(path).unwrap_or_else(|_| PhotoMeta {
        id: scan::photo_id(path),
        path: path.to_string_lossy().to_string(),
        file_name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        stem: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        ext: scan::ext_lower(path).unwrap_or_default(),
        bytes: 0,
        mtime: 0,
        width: 0,
        height: 0,
        orientation: 1,
        taken_at: None,
        sub_sec: None,
        camera: None,
        lens: None,
        iso: None,
        shutter: None,
        shutter_secs: None,
        aperture: None,
        focal_len: None,
    });

    Analysis {
        meta,
        metrics: Metrics::default(),
        faces: Vec::new(),
        subjects: Vec::new(),
        subject_stats: SubjectStats::default(),
        score: 0.0,
        verdict: Verdict::Reject,
        reasons: Vec::new(),
        group_id: None,
        is_group_best: false,
        group_rank: 0,
        user_pick: None,
        rating: 0,
        error: Some(err),
    }
}

/// Decode a larger preview for the loupe/compare view, on demand.
pub fn preview_jpeg(path: &Path, long_edge: u32, orientation: u16) -> Result<Vec<u8>> {
    let rgb = decode::decode_scaled(path, long_edge)?;
    let rgb = decode::apply_orientation(rgb, orientation);
    decode::encode_jpeg(&rgb, 88)
}

/// Ensure a thumbnail exists and return its path, generating it if the cache
/// was cleared behind our back.
pub fn ensure_thumb(folder: &Path, meta: &PhotoMeta) -> Result<PathBuf> {
    let cache = Cache::open(folder)?;
    let thumb = cache.thumb_path(&meta.id, meta.mtime, THUMB_SIZE);
    if thumb.exists() {
        return Ok(thumb);
    }
    let path = PathBuf::from(&meta.path);
    let rgb = decode::decode_scaled(&path, THUMB_SIZE)?;
    let rgb = decode::apply_orientation(rgb, meta.orientation);
    let bytes = decode::encode_jpeg(&rgb, 82)?;
    std::fs::write(&thumb, bytes)?;
    Ok(thumb)
}
