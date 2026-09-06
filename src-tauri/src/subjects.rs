//! Subject detection — what the photograph is actually *of*.
//!
//! This turned out to matter more than any per-pixel measurement. Measured on
//! a real 872-frame football shoot, the frames a photographer calls good have a
//! large subject; the ones they discard have tiny distant figures. Frame-wide
//! sharpness is not merely a weak proxy for that, it is *anti*-correlated with
//! it — a wide record shot is full of in-focus grass and crowd and therefore
//! scores higher than a tight action frame whose background is thrown out.
//!
//! Model is YOLOX-S from the OpenCV Zoo (Apache-2.0, 34 MB, 40.5 mAP on COCO).
//! It was chosen over NanoDet-Plus for two reasons: `tract` cannot load the
//! NanoDet export at all (its `Resize` uses `pytorch_half_pixel`), and on this
//! shoot YOLOX was markedly more confident (0.85-0.92 versus 0.4-0.8) and
//! picked up the ball, which NanoDet missed.

use crate::decode::{resize_rgb, Gray, Rgb8};
use crate::model::{Subject, SubjectStats};
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use tract_onnx::prelude::*;

/// YOLOX-S input resolution.
const NET: usize = 640;
/// Anchor strides, in the order the model concatenates them.
const STRIDES: [usize; 3] = [8, 16, 32];
/// 4 box values + objectness + 80 class scores.
const STRIDE_ROW: usize = 85;
/// Total anchors: 80² + 40² + 20².
const ANCHORS: usize = 8400;
/// Boxes overlapping more than this are treated as the same object.
const NMS_IOU: f32 = 0.45;
/// Objectness below this is not worth the class lookup.
const OBJ_FLOOR: f32 = 0.15;

type Runnable = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

pub struct SubjectDetector {
    model: Runnable,
    /// Precomputed (grid_x, grid_y, stride) per anchor.
    grid: Vec<(f32, f32, f32)>,
}

impl SubjectDetector {
    /// Load YOLOX, searching every candidate model directory.
    pub fn load(app_dir: Option<PathBuf>) -> Result<Self> {
        let path = crate::faces::locate_model(app_dir, "object_detection_yolox_2022nov.onnx")?;
        let model = tract_onnx::onnx()
            .model_for_path(&path)?
            .with_input_fact(0, f32::fact([1, 3, NET, NET]).into())?
            .into_optimized()?
            .into_runnable()?;

        let mut grid = Vec::with_capacity(ANCHORS);
        for stride in STRIDES {
            let cells = NET / stride;
            for y in 0..cells {
                for x in 0..cells {
                    grid.push((x as f32, y as f32, stride as f32));
                }
            }
        }
        debug_assert_eq!(grid.len(), ANCHORS);

        Ok(Self { model, grid })
    }

    /// Detect objects in an oriented RGB frame.
    ///
    /// `gray` is the matching luminance plane, reused to measure per-subject
    /// sharpness without a second conversion.
    pub fn detect(&self, img: &Rgb8, gray: &Gray, min_confidence: f32) -> Result<Vec<Subject>> {
        // YOLOX letterboxes to the *top left* and pads the remainder with 114,
        // unlike YuNet which centres. Getting this wrong shifts every box.
        let scale = (NET as f32 / img.w as f32).min(NET as f32 / img.h as f32);
        let nw = ((img.w as f32 * scale).round() as u32).clamp(1, NET as u32);
        let nh = ((img.h as f32 * scale).round() as u32).clamp(1, NET as u32);
        let resized = resize_rgb(img, nw, nh);

        let plane = NET * NET;
        // Raw 0-255 values in RGB order; YOLOX applies no mean/std.
        let mut chw = vec![114.0f32; plane * 3];
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                let si = (y * nw as usize + x) * 3;
                let di = y * NET + x;
                chw[di] = resized.data[si] as f32;
                chw[plane + di] = resized.data[si + 1] as f32;
                chw[2 * plane + di] = resized.data[si + 2] as f32;
            }
        }

        let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, NET, NET), chw)?.into();
        let outputs = self.model.run(tvec!(tensor.into()))?;
        let view = outputs[0].to_array_view::<f32>()?;
        let data = view
            .as_slice()
            .ok_or_else(|| anyhow!("YOLOX output not contiguous"))?;
        if data.len() < ANCHORS * STRIDE_ROW {
            return Err(anyhow!(
                "YOLOX output too short: {} values for {} anchors",
                data.len(),
                ANCHORS
            ));
        }

        let mut dets: Vec<Det> = Vec::new();
        for i in 0..ANCHORS {
            let row = &data[i * STRIDE_ROW..(i + 1) * STRIDE_ROW];
            let obj = row[4];
            if obj < OBJ_FLOOR {
                continue;
            }
            let (mut best, mut best_c) = (0f32, 0u16);
            for c in 0..80usize {
                let v = row[5 + c];
                if v > best {
                    best = v;
                    best_c = c as u16;
                }
            }
            let score = obj * best;
            if score < min_confidence {
                continue;
            }

            let (gx, gy, st) = self.grid[i];
            let cx = (row[0] + gx) * st;
            let cy = (row[1] + gy) * st;
            let w = row[2].exp() * st;
            let h = row[3].exp() * st;

            dets.push(Det {
                x1: cx - w / 2.0,
                y1: cy - h / 2.0,
                x2: cx + w / 2.0,
                y2: cy + h / 2.0,
                score,
                class_id: best_c,
            });
        }

        let dets = nms(dets, NMS_IOU);
        let (iw, ih) = (img.w as f32, img.h as f32);
        let mut out = Vec::with_capacity(dets.len());

        for d in dets {
            // Undo the letterbox and normalise against the real frame.
            let x = (d.x1 / scale).clamp(0.0, iw);
            let y = (d.y1 / scale).clamp(0.0, ih);
            let x2 = (d.x2 / scale).clamp(0.0, iw);
            let y2 = (d.y2 / scale).clamp(0.0, ih);
            let (bw, bh) = (x2 - x, y2 - y);
            if bw < 2.0 || bh < 2.0 {
                continue;
            }

            let sharp =
                crate::metrics::region_sharpness(gray, x as u32, y as u32, x2 as u32, y2 as u32);

            out.push(Subject {
                class_id: d.class_id,
                label: class_name(d.class_id).to_string(),
                x: x / iw,
                y: y / ih,
                w: bw / iw,
                h: bh / ih,
                score: d.score.clamp(0.0, 1.0),
                area: ((bw * bh) / (iw * ih)).clamp(0.0, 1.0),
                sharpness: crate::metrics::normalise_sharpness(sharp),
                counts: false, // filled in by `summarise`
            });
        }

        out.sort_by(|a, b| {
            b.area
                .partial_cmp(&a.area)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }
}

struct Det {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    class_id: u16,
}

fn iou(a: &Det, b: &Det) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = (a.x2 - a.x1) * (a.y2 - a.y1) + (b.x2 - b.x1) * (b.y2 - b.y1) - inter;
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
        // Class-aware: a ball overlapping a player is not a duplicate of it.
        if keep
            .iter()
            .all(|k| k.class_id != d.class_id || iou(k, &d) < thresh)
        {
            keep.push(d);
        }
    }
    keep
}

/// Mark which detections count for this shoot and summarise them.
///
/// Separated from detection so changing the subject classes re-scores without
/// re-running the network.
pub fn summarise(
    subjects: &mut [Subject],
    classes: &[u16],
    frame_sharpness: f32,
    tile_map: Option<&crate::metrics::SharpnessMap>,
) -> SubjectStats {
    for s in subjects.iter_mut() {
        s.counts = classes.contains(&s.class_id);
    }

    let counting: Vec<&Subject> = subjects.iter().filter(|s| s.counts).collect();
    if counting.is_empty() {
        return SubjectStats {
            main_area: 0.0,
            main_sharpness: 0.0,
            background_sharpness: frame_sharpness,
            count: 0,
            total_area: 0.0,
        };
    }

    // Pick the main subject by size *and* focus, not size alone.
    //
    // On a touchline shoot the biggest box in frame is regularly a coach or
    // spectator standing a couple of metres from the lens, well outside the
    // plane of focus. Letting raw area decide hands the frame's identity to
    // that person and buries the actual action, so a soft box is discounted.
    let main = counting
        .iter()
        .max_by(|a, b| {
            main_rank(a, frame_sharpness)
                .partial_cmp(&main_rank(b, frame_sharpness))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();

    // Background sharpness: mean of tiles that no subject box covers. This is
    // what separates a tight telephoto frame (soft background) from a wide
    // record shot (everything equally sharp).
    let background = tile_map
        .map(|m| background_sharpness(m, subjects))
        .unwrap_or(frame_sharpness);

    SubjectStats {
        main_area: main.area,
        main_sharpness: main.sharpness,
        background_sharpness: background,
        count: counting.len(),
        total_area: counting.iter().map(|s| s.area).sum::<f32>().min(1.0),
    }
}

/// How much a subject deserves to be treated as *the* subject.
fn main_rank(s: &Subject, frame_sharpness: f32) -> f32 {
    let deficit = (frame_sharpness - s.sharpness).max(0.0);
    // Discount, never disqualify: a soft subject is still a subject, and on a
    // frame where nothing is sharp this must not collapse to an arbitrary pick.
    let focus = 1.0 - 0.75 * (deficit / 0.35).clamp(0.0, 1.0);
    prominence(s.area) * focus
}

/// Subject area mapped to 0..1 on a log scale.
///
/// Area spans three orders of magnitude across a shoot: 0.15% is a speck on
/// the far touchline, 3% is a usable subject, 25% fills the frame.
pub fn prominence(area: f32) -> f32 {
    const LO: f32 = -0.82; // log10(0.15)
    const HI: f32 = 1.40; // log10(25)
    let pct = (area * 100.0).max(0.01);
    ((pct.log10() - LO) / (HI - LO)).clamp(0.0, 1.0)
}

fn background_sharpness(map: &crate::metrics::SharpnessMap, subjects: &[Subject]) -> f32 {
    let mut sum = 0f32;
    let mut n = 0u32;
    for ty in 0..map.rows {
        for tx in 0..map.cols {
            // Tile centre in normalised coordinates.
            let cx = (tx as f32 + 0.5) / map.cols as f32;
            let cy = (ty as f32 + 0.5) / map.rows as f32;
            let inside = subjects.iter().any(|s| {
                cx >= s.x && cx <= s.x + s.w && cy >= s.y && cy <= s.y + s.h
            });
            if !inside {
                sum += map.tiles[(ty * map.cols + tx) as usize];
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        crate::metrics::normalise_sharpness(sum / n as f32)
    }
}

/// COCO class names, in model output order.
pub const COCO: [&str; 80] = [
    "person", "bicycle", "car", "motorcycle", "airplane", "bus", "train", "truck", "boat",
    "traffic light", "fire hydrant", "stop sign", "parking meter", "bench", "bird", "cat", "dog",
    "horse", "sheep", "cow", "elephant", "bear", "zebra", "giraffe", "backpack", "umbrella",
    "handbag", "tie", "suitcase", "frisbee", "skis", "snowboard", "sports ball", "kite",
    "baseball bat", "baseball glove", "skateboard", "surfboard", "tennis racket", "bottle",
    "wine glass", "cup", "fork", "knife", "spoon", "bowl", "banana", "apple", "sandwich",
    "orange", "broccoli", "carrot", "hot dog", "pizza", "donut", "cake", "chair", "couch",
    "potted plant", "bed", "dining table", "toilet", "tv", "laptop", "mouse", "remote",
    "keyboard", "cell phone", "microwave", "oven", "toaster", "sink", "refrigerator", "book",
    "clock", "vase", "scissors", "teddy bear", "hair drier", "toothbrush",
];

pub fn class_name(id: u16) -> &'static str {
    COCO.get(id as usize).copied().unwrap_or("object")
}
