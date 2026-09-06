//! Types shared between the analysis pipeline and the React front end.
//!
//! Everything here is serialised straight to the webview, so field names are
//! camelCase on the wire and the TypeScript mirror lives in `src/types.ts`.

use serde::{Deserialize, Serialize};

/// Metadata read from the file system and EXIF, before any pixels are touched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMeta {
    /// Stable id derived from the absolute path; used as a cache key and React key.
    pub id: String,
    pub path: String,
    pub file_name: String,
    /// File name without extension. This is what maps a proxy JPEG to its RAW.
    pub stem: String,
    pub ext: String,
    pub bytes: u64,
    /// File mtime, unix seconds. Part of the cache key.
    pub mtime: i64,
    pub width: u32,
    pub height: u32,
    /// EXIF orientation (1..8). 1 = as stored.
    pub orientation: u16,
    /// Capture time, unix seconds. Drives burst grouping.
    pub taken_at: Option<i64>,
    /// SubSecTimeOriginal, used to order shots inside a burst.
    pub sub_sec: Option<u32>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Human readable, e.g. "1/1600".
    pub shutter: Option<String>,
    /// Exposure time in seconds, for the shake heuristic.
    pub shutter_secs: Option<f32>,
    pub aperture: Option<f32>,
    pub focal_len: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureStats {
    /// Mean luminance, 0..1.
    pub mean: f32,
    /// Standard deviation of luminance, 0..1. A proxy for global contrast.
    pub stddev: f32,
    /// Fraction of pixels at or near 255.
    pub clipped_highlights: f32,
    /// Fraction of pixels at or near 0.
    pub clipped_shadows: f32,
    /// -1 (heavily under) .. 0 (good) .. +1 (heavily over).
    pub bias: f32,
}

/// Objective, per-image measurements. No opinions here, just numbers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// Raw (un-compressed) Laplacian variance at the frame's sharpness peak.
    /// Kept so the scale in `metrics::normalise_sharpness` can be checked
    /// against real photographs rather than assumed.
    pub sharpness_raw: f32,
    /// Variance of the Laplacian over the whole frame, log-compressed.
    pub sharpness_global: f32,
    /// 95th percentile of per-tile sharpness. Survives bokeh backgrounds,
    /// which is what makes it usable on long-lens sports frames.
    pub sharpness_peak: f32,
    /// Sharpness measured inside the subject region (faces when present).
    pub sharpness_subject: f32,
    /// Where the sharpest tile sits, normalised 0..1. Useful for "focused on
    /// the wrong thing" diagnostics.
    pub focus_x: f32,
    pub focus_y: f32,
    /// 0..1. High means gradients are strongly oriented in one direction,
    /// i.e. directional smear rather than uniform defocus.
    pub motion_blur: f32,
    /// Dominant blur direction in degrees, only meaningful when motion_blur is high.
    pub motion_angle: f32,
    pub exposure: ExposureStats,
    /// Rough noise estimate from the high-frequency residual, 0..1.
    pub noise: f32,
    /// Perceptual hash (DCT based), for near-duplicate detection.
    pub phash: u64,
    /// Difference hash, cheap and complementary to phash.
    pub dhash: u64,
    /// Coarse 8x8 grayscale signature, for a refined second-stage compare.
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EyeState {
    Open,
    Squint,
    Closed,
    Unknown,
}

impl Default for EyeState {
    fn default() -> Self {
        EyeState::Unknown
    }
}

/// Where an eye-state verdict came from, so the UI can be honest about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EyeSource {
    /// Photometric heuristic on the eye crop. Always available.
    Heuristic,
    /// Local ONNX classifier.
    Model,
    /// Remote vision model, run only on borderline crops.
    Cloud,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Face {
    /// Bounding box, normalised 0..1 against the oriented image.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Detector confidence 0..1.
    pub score: f32,
    /// right_eye, left_eye, nose, right_mouth, left_mouth — normalised 0..1.
    pub landmarks: Vec<[f32; 2]>,
    /// Confidence that the eyes are open, 0..1.
    pub eye_open: f32,
    pub eye_state: EyeState,
    pub eye_source: EyeSource,
    /// Sharpness measured on this face only, comparable to `sharpness_peak`.
    pub sharpness: f32,
    /// Fraction of the frame this face occupies. Small faces get less weight.
    pub area: f32,
}

/// A detected object — a player, a bird, a bike. This is what the photo is
/// *of*, which turns out to matter far more than any per-pixel measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    /// COCO class index.
    pub class_id: u16,
    /// Human-readable class name.
    pub label: String,
    /// Normalised 0..1 against the oriented frame.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Fraction of the frame this subject covers.
    pub area: f32,
    /// Sharpness measured inside the box, comparable to `sharpness_peak`.
    pub sharpness: f32,
    /// True when this class is one the user is culling for.
    pub counts: bool,
}

/// Frame-level summary of what was detected, so the UI and scorer do not have
/// to re-derive it from the box list every time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectStats {
    /// Area of the largest counting subject, 0..1. The single strongest
    /// predictor of whether a sports frame is worth keeping.
    pub main_area: f32,
    /// Sharpness of that subject.
    pub main_sharpness: f32,
    /// Mean sharpness of the frame outside every subject box.
    pub background_sharpness: f32,
    /// How many counting subjects were found.
    pub count: usize,
    /// Combined area of all counting subjects.
    pub total_area: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Keep,
    Maybe,
    Reject,
}

impl Default for Verdict {
    fn default() -> Self {
        Verdict::Maybe
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Good,
    Warn,
    Bad,
}

/// A human-readable justification attached to a photo. The UI shows these
/// verbatim, so they carry the burden of explaining the score.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    /// Stable machine key, e.g. "soft", "eyes-closed", "duplicate".
    pub code: String,
    pub label: String,
    pub severity: Severity,
    /// Signed contribution to the final score, in points.
    pub delta: f32,
}

/// A user's manual override. Always wins over the computed verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pick {
    Keep,
    Reject,
}

/// The full per-photo record. This is what the grid renders and what the
/// exporter reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub meta: PhotoMeta,
    pub metrics: Metrics,
    pub faces: Vec<Face>,
    pub subjects: Vec<Subject>,
    pub subject_stats: SubjectStats,
    /// 0..100. Higher is better.
    pub score: f32,
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
    /// Index of the burst/near-duplicate group this photo belongs to.
    pub group_id: Option<u32>,
    /// True when this is the best frame of its group.
    pub is_group_best: bool,
    /// Rank within the group, 0 = best.
    pub group_rank: u32,
    /// Manual override, set from the UI.
    pub user_pick: Option<Pick>,
    /// Star rating 0..5, exported into XMP sidecars.
    pub rating: u8,
    /// Set when analysis failed; the photo still appears in the grid.
    pub error: Option<String>,
}

impl Analysis {
    /// The verdict the exporter should honour: manual override, else computed.
    pub fn effective_keep(&self) -> bool {
        match self.user_pick {
            Some(Pick::Keep) => true,
            Some(Pick::Reject) => false,
            None => self.verdict == Verdict::Keep,
        }
    }
}

/// Tunables the user can move in the sidebar. Re-scoring with new settings is
/// cheap because it never re-reads pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreSettings {
    /// Below this normalised sharpness a photo is called soft.
    pub sharpness_floor: f32,
    /// Score at or above which a photo is a Keep.
    pub keep_threshold: f32,
    /// Score below which a photo is a Reject.
    pub reject_threshold: f32,
    /// Weight of each component, 0..1.
    pub w_sharpness: f32,
    pub w_exposure: f32,
    pub w_faces: f32,
    pub w_composition: f32,
    /// How much the subject drives the score, 0 (ignore it, judge the frame on
    /// technical merit alone) .. 1 (the subject is the photo).
    pub w_subject: f32,
    /// COCO class ids that count as "the subject" for this shoot.
    pub subject_classes: Vec<u16>,
    /// Detections below this confidence are ignored.
    pub subject_confidence: f32,
    /// Max seconds between frames for them to count as the same burst.
    pub burst_gap_secs: f32,
    /// Require frames to be close in capture time to group.
    pub group_by_time: bool,
    /// Require frames to look alike to group.
    pub group_by_appearance: bool,
    /// Max Hamming distance between pHashes for near-duplicate grouping.
    pub hash_distance: u32,
    /// Demote every frame in a group except the best one.
    pub keep_one_per_group: bool,
    /// Treat closed eyes as disqualifying rather than a penalty.
    pub strict_eyes: bool,
}

impl Default for ScoreSettings {
    fn default() -> Self {
        Self {
            sharpness_floor: 0.32,
            // Tuned against the observed score distribution: with sharpness as
            // the baseline and everything else deducting, a clean shoot lands
            // roughly 61-98, so these put the softest fifth into Maybe and
            // reserve Reject for frames with a real defect.
            keep_threshold: 72.0,
            reject_threshold: 50.0,
            w_sharpness: 1.0,
            w_exposure: 0.6,
            // Eyes are a weak signal in field sport — players rarely face the
            // camera and their faces are small. Kept available, weighted low.
            w_faces: 0.35,
            w_composition: 0.3,
            // The subject dominates by default. Without this, frame-wide
            // sharpness wins, and frame-wide sharpness is *highest* on wide
            // record shots full of in-focus grass — the exact frames you do
            // not want at the top.
            w_subject: 0.8,
            // People, and the things people chase.
            subject_classes: vec![0, 32],
            subject_confidence: 0.35,
            // Measured on a real 872-frame shoot: continuous drive fires every
            // 0.06-0.12 s, then there is a near-empty band before 0.25 s, so a
            // sub-second window separates "same burst" from "shutter pressed
            // again" almost perfectly. The old 2 s default merged runs of up to
            // 54 frames and, on time alone, joined visibly different pictures
            // 24% of the time.
            burst_gap_secs: 0.5,
            group_by_time: true,
            group_by_appearance: true,
            hash_distance: 10,
            keep_one_per_group: true,
            strict_eyes: false,
        }
    }
}

/// Emitted on the `analysis:progress` channel while a run is in flight.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub stage: String,
    pub elapsed_ms: u64,
    /// Estimated milliseconds remaining, once there is enough data to guess.
    pub eta_ms: Option<u64>,
}
