//! Face detection (YuNet) and eye-state estimation.
//!
//! Detection uses YuNet from the OpenCV Zoo — 227 KB, Apache-2.0, and
//! genuinely good on small faces, which matters when the subject is 60 px tall
//! at the far end of the pitch.
//!
//! Eye state is the harder half. There is no small, freely redistributable,
//! reliably-hosted open/closed classifier, so the always-available path here is
//! a photometric heuristic on the eye crop. It is deliberately conservative:
//! it reports `Unknown` rather than guessing when the crop is too small or too
//! soft to judge, and the UI shows which source produced each verdict. Drop an
//! ONNX classifier at `models/eye_state.onnx` and it is used instead; borderline
//! crops can additionally be sent to a vision model (see `cloud.rs`).

use crate::decode::{crop_rgb, resize_rgb, Gray, Rgb8};
use crate::model::{EyeSource, EyeState, Face};
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use tract_onnx::prelude::*;

/// YuNet's fixed input resolution.
const NET: usize = 640;
/// Score below which a detection is discarded outright.
const SCORE_THRESHOLD: f32 = 0.55;
/// IoU above which two boxes are considered the same face.
const NMS_IOU: f32 = 0.35;
/// Graph output order, as emitted by the 2023mar YuNet export.
const OUTPUT_ORDER: [&str; 12] = [
    "cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16", "bbox_32",
    "kps_8", "kps_16", "kps_32",
];

type Runnable = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

pub struct FaceDetector {
    model: Runnable,
}

/// Letterbox parameters, needed to map detections back to image coordinates.
struct Letterbox {
    scale: f32,
    dx: f32,
    dy: f32,
}

impl FaceDetector {
    /// Load YuNet, searching every candidate model directory.
    pub fn load(app_dir: Option<PathBuf>) -> Result<Self> {
        let path = locate_model(app_dir, "face_detection_yunet_2023mar.onnx")?;
        let model = tract_onnx::onnx()
            .model_for_path(&path)?
            .with_input_fact(0, f32::fact([1, 3, NET, NET]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Self { model })
    }

    /// Detect faces in an already-oriented RGB image.
    ///
    /// `gray` must be the luminance of the same image; it is reused to measure
    /// per-face sharpness without a second conversion.
    pub fn detect(&self, img: &Rgb8, gray: &Gray) -> Result<Vec<Face>> {
        let (input, lb) = letterbox(img);

        let tensor: Tensor =
            tract_ndarray::Array4::from_shape_vec((1, 3, NET, NET), input)?.into();
        let outputs = self.model.run(tvec!(tensor.into()))?;
        if outputs.len() != OUTPUT_ORDER.len() {
            return Err(anyhow!(
                "unexpected YuNet output count: {} (expected {})",
                outputs.len(),
                OUTPUT_ORDER.len()
            ));
        }

        let mut views = Vec::with_capacity(outputs.len());
        for out in outputs.iter() {
            views.push(out.to_array_view::<f32>()?);
        }

        let mut dets: Vec<Det> = Vec::new();
        for (si, &stride) in [8usize, 16, 32].iter().enumerate() {
            let cells = NET / stride;
            let n = cells * cells;
            let cls = views[si].as_slice().ok_or_else(|| anyhow!("cls not contiguous"))?;
            let obj = views[3 + si].as_slice().ok_or_else(|| anyhow!("obj not contiguous"))?;
            let bbox = views[6 + si].as_slice().ok_or_else(|| anyhow!("bbox not contiguous"))?;
            let kps = views[9 + si].as_slice().ok_or_else(|| anyhow!("kps not contiguous"))?;

            if cls.len() < n || obj.len() < n || bbox.len() < n * 4 || kps.len() < n * 10 {
                return Err(anyhow!("YuNet output shorter than {} anchors at stride {}", n, stride));
            }

            for i in 0..n {
                // YuNet's confidence is the geometric mean of objectness and class score.
                let score = (cls[i].max(0.0) * obj[i].max(0.0)).sqrt();
                if score < SCORE_THRESHOLD {
                    continue;
                }
                let col = (i % cells) as f32;
                let row = (i / cells) as f32;
                let s = stride as f32;

                let cx = (col + bbox[i * 4]) * s;
                let cy = (row + bbox[i * 4 + 1]) * s;
                let w = bbox[i * 4 + 2].exp() * s;
                let h = bbox[i * 4 + 3].exp() * s;

                let mut lm = [[0f32; 2]; 5];
                for k in 0..5 {
                    lm[k][0] = (col + kps[i * 10 + k * 2]) * s;
                    lm[k][1] = (row + kps[i * 10 + k * 2 + 1]) * s;
                }

                dets.push(Det {
                    x: cx - w / 2.0,
                    y: cy - h / 2.0,
                    w,
                    h,
                    score,
                    lm,
                });
            }
        }

        let dets = nms(dets, NMS_IOU);
        let (iw, ih) = (img.w as f32, img.h as f32);

        let mut faces = Vec::with_capacity(dets.len());
        for d in dets {
            // Undo the letterbox, then normalise against the real image.
            let px = (d.x - lb.dx) / lb.scale;
            let py = (d.y - lb.dy) / lb.scale;
            let pw = d.w / lb.scale;
            let ph = d.h / lb.scale;

            // Reject boxes that landed mostly in the letterbox padding.
            if px + pw < 0.0 || py + ph < 0.0 || px > iw || py > ih {
                continue;
            }

            let mut landmarks = Vec::with_capacity(5);
            let mut lm_px = [[0f32; 2]; 5];
            for k in 0..5 {
                let lx = (d.lm[k][0] - lb.dx) / lb.scale;
                let ly = (d.lm[k][1] - lb.dy) / lb.scale;
                lm_px[k] = [lx, ly];
                landmarks.push([(lx / iw).clamp(0.0, 1.0), (ly / ih).clamp(0.0, 1.0)]);
            }

            let sharp = crate::metrics::laplacian_variance(
                gray,
                px.max(0.0) as u32,
                py.max(0.0) as u32,
                (px + pw).clamp(0.0, iw) as u32,
                (py + ph).clamp(0.0, ih) as u32,
            );

            let (eye_open, eye_state, eye_source) = eye_state(img, &lm_px, pw);

            faces.push(Face {
                x: (px / iw).clamp(0.0, 1.0),
                y: (py / ih).clamp(0.0, 1.0),
                w: (pw / iw).clamp(0.0, 1.0),
                h: (ph / ih).clamp(0.0, 1.0),
                score: d.score.clamp(0.0, 1.0),
                landmarks,
                eye_open,
                eye_state,
                eye_source,
                sharpness: crate::metrics::normalise_sharpness(sharp),
                area: ((pw * ph) / (iw * ih)).clamp(0.0, 1.0),
            });
        }

        // Biggest face first — that is almost always the subject.
        faces.sort_by(|a, b| {
            (b.area * b.score)
                .partial_cmp(&(a.area * a.score))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(faces)
    }
}

struct Det {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    score: f32,
    lm: [[f32; 2]; 5],
}

/// Resize preserving aspect ratio and pad to 640x640, returning CHW BGR floats.
///
/// Aspect must be preserved: squashing a 3:2 frame into a square distorts faces
/// enough to cost real detections on small subjects.
fn letterbox(img: &Rgb8) -> (Vec<f32>, Letterbox) {
    let scale = (NET as f32 / img.w as f32).min(NET as f32 / img.h as f32);
    let nw = ((img.w as f32 * scale).round() as u32).clamp(1, NET as u32);
    let nh = ((img.h as f32 * scale).round() as u32).clamp(1, NET as u32);
    let resized = resize_rgb(img, nw, nh);

    let dx = ((NET as u32 - nw) / 2) as usize;
    let dy = ((NET as u32 - nh) / 2) as usize;

    // Pad with mid grey so the border does not read as a hard edge.
    let plane = NET * NET;
    let mut chw = vec![114.0f32; plane * 3];
    for y in 0..nh as usize {
        for x in 0..nw as usize {
            let si = (y * nw as usize + x) * 3;
            let di = (y + dy) * NET + (x + dx);
            // YuNet was trained on BGR.
            chw[di] = resized.data[si + 2] as f32;
            chw[plane + di] = resized.data[si + 1] as f32;
            chw[2 * plane + di] = resized.data[si] as f32;
        }
    }

    (
        chw,
        Letterbox {
            scale,
            dx: dx as f32,
            dy: dy as f32,
        },
    )
}

fn iou(a: &Det, b: &Det) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn nms(mut dets: Vec<Det>, thresh: f32) -> Vec<Det> {
    dets.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep: Vec<Det> = Vec::new();
    for d in dets {
        if keep.iter().all(|k| iou(k, &d) < thresh) {
            keep.push(d);
        }
    }
    keep
}

/// Estimate whether the eyes are open.
///
/// Returns `(openness 0..1, state, source)`.
///
/// The signal we look for is that an open eye contains a compact dark blob (the
/// iris and pupil) surrounded by much brighter sclera, giving a high-contrast,
/// roughly round dark region. A closed eye is a smooth lid, sometimes with a
/// thin horizontal lash line — low contrast, and any dark region is wide and
/// flat rather than round.
fn eye_state(img: &Rgb8, lm_px: &[[f32; 2]; 5], face_w: f32) -> (f32, EyeState, EyeSource) {
    let (rx, ry) = (lm_px[0][0], lm_px[0][1]);
    let (lx, ly) = (lm_px[1][0], lm_px[1][1]);
    let sep = ((lx - rx).powi(2) + (ly - ry).powi(2)).sqrt().max(face_w * 0.25);

    // Below roughly 22 px of eye separation there is not enough detail at our
    // working resolution to say anything honest. Guessing here was the single
    // biggest source of false "eyes closed" calls: a small, soft face gives a
    // flat, low-contrast crop that superficially resembles an eyelid.
    if sep < 22.0 {
        return (0.5, EyeState::Unknown, EyeSource::Heuristic);
    }

    let cw = (sep * 0.42).round().max(9.0) as u32;
    let ch = (sep * 0.30).round().max(7.0) as u32;

    let mut scores = Vec::new();
    for &(ex, ey) in &[(rx, ry), (lx, ly)] {
        let crop = crop_rgb(
            img,
            (ex - cw as f32 / 2.0).round() as i32,
            (ey - ch as f32 / 2.0).round() as i32,
            cw,
            ch,
        );
        if crop.w < 8 || crop.h < 6 {
            continue;
        }
        if let Some(s) = eye_openness(&crop) {
            scores.push(s);
        }
    }

    // If neither eye could be read, say so rather than inventing a verdict.
    if scores.is_empty() {
        return (0.5, EyeState::Unknown, EyeSource::Heuristic);
    }

    // A blink closes both eyes together, so the *lower* of the two is the
    // honest summary — one clearly closed eye means the frame is a blink.
    let open = scores.iter().cloned().fold(f32::INFINITY, f32::min);

    // Deliberately asymmetric. Calling a good frame a blink is far more costly
    // than missing one: the user loses a keeper and stops trusting the tool,
    // whereas a missed blink merely survives to be looked at. So `Closed`
    // requires strong evidence, and everything uncertain lands in `Unknown`,
    // where the optional cloud pass can pick it up.
    let state = if open >= 0.58 {
        EyeState::Open
    } else if open <= 0.20 {
        EyeState::Closed
    } else if open <= 0.34 {
        EyeState::Squint
    } else {
        EyeState::Unknown
    };

    (open, state, EyeSource::Heuristic)
}

/// Openness score for a single eye crop, 0 (closed) .. 1 (wide open).
///
/// Returns `None` when the crop carries too little information to judge —
/// which is a genuinely different answer from "closed", and treating the two
/// as the same is what made an earlier version call two thirds of all faces
/// blinks.
fn eye_openness(crop: &Rgb8) -> Option<f32> {
    let n = (crop.w * crop.h) as usize;
    let mut lum: Vec<f32> = Vec::with_capacity(n);
    for px in crop.data.chunks_exact(3).take(n) {
        lum.push((0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0);
    }
    if lum.len() < 48 {
        return None;
    }

    let mean = lum.iter().sum::<f32>() / lum.len() as f32;
    let var = lum.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / lum.len() as f32;
    let std = var.sqrt();

    // A very flat crop means we cannot see the eye — motion blur, defocus, or
    // a landmark that missed. That is not evidence of a closed eye.
    if std < 0.05 {
        return None;
    }

    // Otsu split into "dark" (iris, pupil, lashes) and "light" (sclera, skin).
    let thresh = otsu(&lum);
    let dark: Vec<usize> = (0..lum.len()).filter(|&i| lum[i] <= thresh).collect();
    let dark_ratio = dark.len() as f32 / lum.len() as f32;

    // An open eye devotes a meaningful but not overwhelming share of the crop
    // to iris. Outside this band we are not looking at an eye at all.
    if !(0.05..=0.70).contains(&dark_ratio) {
        return None;
    }

    // Shape of the dark region: round implies iris, flat implies lash line.
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for &i in &dark {
        let x = (i as u32) % crop.w;
        let y = (i as u32) / crop.w;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let dw = (x1.saturating_sub(x0) + 1) as f32;
    let dh = (y1.saturating_sub(y0) + 1) as f32;

    // Only the *shape* of the dark region separates an open eye from a closed
    // one, and getting this wrong is easy.
    //
    // An earlier version also added credit for dark/light separation, fill
    // ratio and contrast. All three are just as high for a closed eye — a lash
    // line is dark against skin, it fills its bounding box completely, and it
    // has plenty of contrast. Their combined weight put a hard floor of 0.58
    // under every score, which sat exactly on the "eyes open" threshold, so
    // the classifier could not report a blink at all. It never fired once
    // across an 872-frame shoot.
    //
    // So shape now carries the verdict, measured two ways:
    //   - how tall the dark region is relative to its own width (an iris is
    //     roughly round, a lash line is a flat sliver), and
    //   - how much of the eye opening it spans vertically.
    let aspect = (dh / dw.max(1.0)).clamp(0.0, 2.0);
    let vertical_span = dh / crop.h.max(1) as f32;

    // Contrast is kept only as a small confidence term: it cannot distinguish
    // open from closed, but a crop with none of it is not worth trusting.
    let s_std = ((std - 0.05) / 0.10).clamp(0.0, 1.0);

    let s_aspect = smoothstep(aspect, 0.20, 0.60);
    let s_span = smoothstep(vertical_span, 0.15, 0.55);

    Some((0.55 * s_aspect + 0.35 * s_span + 0.10 * s_std).clamp(0.0, 1.0))
}

/// Hermite ramp from 0 at `lo` to 1 at `hi`.
fn smoothstep(v: f32, lo: f32, hi: f32) -> f32 {
    if hi <= lo {
        return 0.0;
    }
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Otsu's threshold over a 64-bin histogram of 0..1 values.
fn otsu(values: &[f32]) -> f32 {
    const BINS: usize = 64;
    let mut hist = [0u32; BINS];
    for &v in values {
        let b = ((v * (BINS - 1) as f32).round().clamp(0.0, (BINS - 1) as f32)) as usize;
        hist[b] += 1;
    }
    let total = values.len() as f32;
    let sum: f32 = (0..BINS).map(|i| i as f32 * hist[i] as f32).sum();

    let (mut sum_b, mut w_b, mut best_var, mut best_t) = (0f32, 0f32, -1f32, 0usize);
    for t in 0..BINS {
        w_b += hist[t] as f32;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f32 * hist[t] as f32;
        let m_b = sum_b / w_b;
        let m_f = (sum - sum_b) / w_f;
        let between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if between > best_var {
            best_var = between;
            best_t = t;
        }
    }
    // The loop splits bins `0..=best_t` from `best_t+1..`, so the boundary sits
    // at the *upper* edge of bin `best_t`. Returning the bin's own centre
    // instead excludes the darkest pixels from the dark class when callers test
    // `value <= threshold` — on a high-contrast crop that empties the dark set
    // entirely and the eye becomes unjudgeable.
    (best_t as f32 + 0.5) / (BINS - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a greyscale test crop from a per-pixel function.
    fn synth(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Rgb8 {
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                data.extend_from_slice(&[v, v, v]);
            }
        }
        Rgb8 { w, h, data }
    }

    /// Bright sclera with a dark, roughly circular iris.
    fn open_eye() -> Rgb8 {
        synth(40, 28, |x, y| {
            let dx = x as f32 - 20.0;
            let dy = (y as f32 - 14.0) * 1.3;
            if (dx * dx + dy * dy).sqrt() < 7.0 {
                25
            } else {
                215
            }
        })
    }

    /// A smooth lid with a thin dark lash line across it.
    fn closed_eye() -> Rgb8 {
        synth(40, 28, |_x, y| {
            if (y as f32 - 14.0).abs() < 1.5 {
                40
            } else {
                190
            }
        })
    }

    /// The regression this guards against is not hypothetical: the previous
    /// scoring gave a closed eye 0.58, identical to the open-eye threshold, so
    /// no blink was ever reported.
    #[test]
    fn open_and_closed_eyes_are_separated() {
        let o = eye_openness(&open_eye()).expect("open eye should be judgeable");
        let c = eye_openness(&closed_eye()).expect("closed eye should be judgeable");

        assert!(o > 0.58, "open eye scored {o:.2}, would not be called open");
        assert!(c <= 0.20, "closed eye scored {c:.2}, would not be called closed");
        assert!(o - c > 0.40, "open {o:.2} and closed {c:.2} are too close");
    }

    /// A crop with no usable detail must say so rather than guessing "closed",
    /// which is what made an early version call two thirds of faces blinks.
    #[test]
    fn featureless_crop_is_not_a_blink() {
        assert!(eye_openness(&synth(40, 28, |_, _| 128)).is_none());
        // Gentle gradient, still no eye in it.
        assert!(eye_openness(&synth(40, 28, |x, _| (120 + x / 8) as u8)).is_none());
    }

    #[test]
    fn crops_too_small_to_judge_are_rejected() {
        assert!(eye_openness(&synth(5, 4, |_, _| 100)).is_none());
    }

    #[test]
    fn smoothstep_is_clamped_and_monotonic() {
        assert_eq!(smoothstep(0.0, 0.2, 0.6), 0.0);
        assert_eq!(smoothstep(1.0, 0.2, 0.6), 1.0);
        assert!(smoothstep(0.3, 0.2, 0.6) < smoothstep(0.5, 0.2, 0.6));
        // Degenerate bounds must not divide by zero.
        assert_eq!(smoothstep(0.5, 0.4, 0.4), 0.0);
    }
}

/// Where the bundled models live. Kept next to the executable in a release
/// build, and under `src-tauri/models` while developing.
pub fn model_dir(app_dir: Option<PathBuf>) -> PathBuf {
    model_dirs(app_dir)
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("models"))
}

/// Every directory that might hold a model, in priority order.
///
/// Bundled app first, then beside the executable (a portable copy), then the
/// source tree. The source tree only exists on the machine that built it, so
/// without the middle entry a copied build silently loses every model.
pub fn model_dirs(app_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(d) = app_dir {
        out.push(d.join("models"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("models"));
        }
    }
    out.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));
    out.dedup();
    out
}

/// Find one model file across the candidate directories.
///
/// Searching per *file* rather than picking the first directory that exists
/// matters: the build copies resources next to the executable, so a stale
/// `models/` folder there can shadow the real one and hide a model that is
/// present in the source tree.
pub fn locate_model(app_dir: Option<PathBuf>, file: &str) -> Result<PathBuf> {
    let dirs = model_dirs(app_dir);
    for d in &dirs {
        let candidate = d.join(file);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "{file} not found. Looked in: {}. Run `npm run fetch-models`.",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}
