//! Objective image measurements: sharpness, exposure, motion smear, noise.
//!
//! The important idea here is that a *global* sharpness number is close to
//! useless on sports photography. A 400 mm frame is mostly out-of-focus grass
//! and crowd; averaging over that buries the one thing you care about, which is
//! whether the player is sharp. So we measure sharpness per tile and look at
//! the top of the distribution, and separately inside any detected face.

use crate::decode::Gray;
use crate::model::ExposureStats;

/// Tiles across the long edge when building the sharpness map.
const TILE_GRID: u32 = 16;

/// Per-tile sharpness map plus the summary statistics drawn from it.
pub struct SharpnessMap {
    pub tiles: Vec<f32>,
    pub cols: u32,
    pub rows: u32,
    /// Mean over all tiles.
    pub global: f32,
    /// 95th percentile — "how sharp is the sharpest real content".
    pub peak: f32,
    /// Normalised position of the sharpest tile.
    pub focus_x: f32,
    pub focus_y: f32,
}

/// Variance of the Laplacian, computed per tile.
///
/// The Laplacian kernel used is the 8-neighbour form, which responds to both
/// axes equally and is less direction-biased than the 4-neighbour version.
pub fn sharpness_map(gray: &Gray) -> SharpnessMap {
    let (w, h) = (gray.w, gray.h);
    if w < 8 || h < 8 {
        return SharpnessMap {
            tiles: vec![0.0],
            cols: 1,
            rows: 1,
            global: 0.0,
            peak: 0.0,
            focus_x: 0.5,
            focus_y: 0.5,
        };
    }

    // Keep tiles roughly square regardless of aspect ratio.
    let cols = TILE_GRID.max(1);
    let rows = ((h as f32 / w as f32) * cols as f32).round().max(1.0) as u32;
    let tw = (w / cols).max(1);
    let th = (h / rows).max(1);

    let mut tiles = vec![0f32; (cols * rows) as usize];

    for ty in 0..rows {
        for tx in 0..cols {
            let x0 = tx * tw;
            let y0 = ty * th;
            let x1 = if tx == cols - 1 { w } else { x0 + tw };
            let y1 = if ty == rows - 1 { h } else { y0 + th };

            tiles[(ty * cols + tx) as usize] = laplacian_variance(gray, x0, y0, x1, y1);
        }
    }

    let global = if tiles.is_empty() {
        0.0
    } else {
        tiles.iter().sum::<f32>() / tiles.len() as f32
    };

    let mut sorted = tiles.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let peak = percentile(&sorted, 0.95);

    let (mut best_i, mut best_v) = (0usize, f32::MIN);
    for (i, v) in tiles.iter().enumerate() {
        if *v > best_v {
            best_v = *v;
            best_i = i;
        }
    }
    let bx = (best_i as u32 % cols) as f32;
    let by = (best_i as u32 / cols) as f32;

    SharpnessMap {
        tiles,
        cols,
        rows,
        global,
        peak,
        focus_x: (bx + 0.5) / cols as f32,
        focus_y: (by + 0.5) / rows as f32,
    }
}

/// Laplacian variance over a rectangle. Returns 0 for degenerate regions.
pub fn laplacian_variance(gray: &Gray, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let x0 = x0.max(1);
    let y0 = y0.max(1);
    let x1 = x1.min(gray.w.saturating_sub(1));
    let y1 = y1.min(gray.h.saturating_sub(1));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }

    let mut sum = 0f64;
    let mut sum_sq = 0f64;
    let mut n = 0u64;

    for y in y0..y1 {
        for x in x0..x1 {
            let c = gray.at(x, y);
            let lap = 8.0 * c
                - gray.at(x - 1, y - 1)
                - gray.at(x, y - 1)
                - gray.at(x + 1, y - 1)
                - gray.at(x - 1, y)
                - gray.at(x + 1, y)
                - gray.at(x - 1, y + 1)
                - gray.at(x, y + 1)
                - gray.at(x + 1, y + 1);
            sum += lap as f64;
            sum_sq += (lap * lap) as f64;
            n += 1;
        }
    }

    if n < 2 {
        return 0.0;
    }
    let mean = sum / n as f64;
    let var = (sum_sq / n as f64) - mean * mean;
    var.max(0.0) as f32
}

/// Lower end of the useful log10(variance) range — visually mush.
///
/// Calibrated at `pipeline::WORK_SIZE`. Measured over an 872-frame Canon R5
/// shoot at 3072 px, the competently-focused population runs from about -1.98
/// (2nd percentile) to -1.01 (98th). Genuinely missed focus sits well below
/// that, so the floor is set clear of the good population rather than inside
/// it — the point of the absolute scale is to accuse only when it is sure.
///
/// Note this is resolution-dependent: at a smaller working size, aliasing
/// raises the Laplacian response and shifts the whole distribution up. Re-run
/// `examples/cull.rs` if `WORK_SIZE` ever changes.
const SHARP_LOG_LO: f32 = -3.0;
/// Upper end — crisp, well-resolved detail. Above this the differences stop
/// mattering to a human looking at the frame.
const SHARP_LOG_HI: f32 = -0.9;

/// Sharpness of a region, judged the way the frame is judged: tiled, then a
/// high percentile.
///
/// A plain variance over the whole box is biased against large subjects. Most
/// of a person's bounding box is smooth clothing, so a tack-sharp player
/// filling a quarter of the frame measures *softer* than a distant one whose
/// box is mostly grass. What matters for focus is whether crisp detail exists
/// somewhere on the subject — an edge, a face, a number — so take the best
/// tiles rather than the average.
pub fn region_sharpness(gray: &Gray, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w < 12 || h < 12 {
        // Too small to subdivide meaningfully; the plain variance is all we have.
        return laplacian_variance(gray, x0, y0, x1, y1);
    }

    // Aim for tiles around 24 px so even a modest box yields a few samples.
    let cols = (w / 24).clamp(2, 6);
    let rows = (h / 24).clamp(2, 6);
    let tw = (w / cols).max(1);
    let th = (h / rows).max(1);

    let mut tiles = Vec::with_capacity((cols * rows) as usize);
    for ty in 0..rows {
        for tx in 0..cols {
            let sx = x0 + tx * tw;
            let sy = y0 + ty * th;
            let ex = if tx == cols - 1 { x1 } else { sx + tw };
            let ey = if ty == rows - 1 { y1 } else { sy + th };
            tiles.push(laplacian_variance(gray, sx, sy, ex, ey));
        }
    }
    tiles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    percentile(&tiles, 0.85)
}

/// Compress raw Laplacian variance into a friendlier 0..1 range.
///
/// Variance spans several orders of magnitude, so a log curve is the only way
/// to get a scale where the difference between "soft" and "usable" is visible.
///
/// The bounds are calibrated against 2048 px decodes of 45 MP frames — see
/// `examples/cull.rs`, which prints the raw distribution so this can be
/// re-checked against real photographs rather than assumed.
pub fn normalise_sharpness(variance: f32) -> f32 {
    if variance <= 0.0 {
        return 0.0;
    }
    let l = variance.log10();
    ((l - SHARP_LOG_LO) / (SHARP_LOG_HI - SHARP_LOG_LO)).clamp(0.0, 1.0)
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f32 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Luminance histogram statistics.
pub fn exposure(gray: &Gray) -> ExposureStats {
    let n = gray.data.len();
    if n == 0 {
        return ExposureStats::default();
    }

    let mut hist = [0u32; 256];
    let mut sum = 0f64;
    for &v in &gray.data {
        let b = (v * 255.0).round().clamp(0.0, 255.0) as usize;
        hist[b] += 1;
        sum += v as f64;
    }
    let mean = (sum / n as f64) as f32;

    let mut var = 0f64;
    for &v in &gray.data {
        let d = (v - mean) as f64;
        var += d * d;
    }
    let stddev = ((var / n as f64).sqrt()) as f32;

    // "Clipped" is deliberately loose: 250+ and 5- rather than exactly 255/0,
    // because JPEG quantisation smears the true clipping point.
    let hi: u32 = hist[250..].iter().sum();
    let lo: u32 = hist[..6].iter().sum();
    let clipped_highlights = hi as f32 / n as f32;
    let clipped_shadows = lo as f32 / n as f32;

    // A well-exposed frame sits near 0.45 mean. Map the deviation to -1..1.
    let bias = ((mean - 0.45) / 0.45).clamp(-1.0, 1.0);

    ExposureStats {
        mean,
        stddev,
        clipped_highlights,
        clipped_shadows,
        bias,
    }
}

/// Detect directional smear.
///
/// Motion blur leaves gradients concentrated perpendicular to the direction of
/// travel, while defocus spreads them evenly. We bin gradient orientations by
/// magnitude and measure how peaked the resulting distribution is. Returns
/// `(anisotropy 0..1, dominant angle in degrees)`.
pub fn motion_blur(gray: &Gray) -> (f32, f32) {
    let (w, h) = (gray.w, gray.h);
    if w < 4 || h < 4 {
        return (0.0, 0.0);
    }

    const BINS: usize = 36; // 5 degrees per bin over 180 degrees
    let mut hist = [0f64; BINS];
    let mut total = 0f64;

    // Subsample; orientation statistics converge quickly and this keeps the
    // pass cheap on large images.
    let step = ((w.max(h) / 512).max(1)) as u32;

    for y in (1..h - 1).step_by(step as usize) {
        for x in (1..w - 1).step_by(step as usize) {
            let gx = gray.at(x + 1, y) - gray.at(x - 1, y);
            let gy = gray.at(x, y + 1) - gray.at(x, y - 1);
            let mag = (gx * gx + gy * gy).sqrt();
            if mag < 0.02 {
                continue; // ignore flat areas and sensor noise
            }
            // Orientation is modulo 180 degrees: a line has no direction.
            let mut ang = gy.atan2(gx).to_degrees();
            if ang < 0.0 {
                ang += 180.0;
            }
            let bin = ((ang / 180.0) * BINS as f32) as usize % BINS;
            hist[bin] += mag as f64;
            total += mag as f64;
        }
    }

    if total <= 0.0 {
        return (0.0, 0.0);
    }

    // Circular variance over doubled angles gives a clean 0..1 concentration
    // measure that respects the 180-degree wraparound.
    let mut sx = 0f64;
    let mut sy = 0f64;
    for (i, &v) in hist.iter().enumerate() {
        let ang = (i as f64 + 0.5) / BINS as f64 * std::f64::consts::PI;
        sx += v * (2.0 * ang).cos();
        sy += v * (2.0 * ang).sin();
    }
    let r = ((sx * sx + sy * sy).sqrt() / total).clamp(0.0, 1.0);
    let mut angle = (sy.atan2(sx).to_degrees() / 2.0) as f32;
    if angle < 0.0 {
        angle += 180.0;
    }

    // Even a clean image shows some anisotropy from horizons and verticals, so
    // rescale to make the interesting range occupy most of 0..1.
    let aniso = (((r as f32) - 0.15) / 0.55).clamp(0.0, 1.0);
    (aniso, angle)
}

/// Rough noise estimate from the median absolute deviation of a high-pass
/// residual. Correlates well enough with ISO to catch pushed frames.
pub fn noise(gray: &Gray) -> f32 {
    let (w, h) = (gray.w, gray.h);
    if w < 8 || h < 8 {
        return 0.0;
    }
    let step = ((w.max(h) / 256).max(1)) as usize;
    let mut samples: Vec<f32> = Vec::new();

    for y in (1..h - 1).step_by(step) {
        for x in (1..w - 1).step_by(step) {
            // Second difference along x kills smooth gradients and edges of
            // moderate slope, leaving mostly grain.
            let d = gray.at(x - 1, y) - 2.0 * gray.at(x, y) + gray.at(x + 1, y);
            samples.push(d.abs());
        }
    }
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = samples[samples.len() / 2];
    // 0.02 residual is already quite grainy at our working resolution.
    (mad / 0.02).clamp(0.0, 1.0)
}

/// Fraction of the frame that is well exposed and not flat. Used as a weak
/// composition signal — badly framed shots often show up as very low variance.
pub fn detail_coverage(map: &SharpnessMap) -> f32 {
    if map.tiles.is_empty() || map.peak <= 0.0 {
        return 0.0;
    }
    let thresh = map.peak * 0.25;
    let n = map.tiles.iter().filter(|t| **t >= thresh).count();
    n as f32 / map.tiles.len() as f32
}
