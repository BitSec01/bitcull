//! Optional cloud verification of borderline eye-state calls.
//!
//! This is deliberately narrow. Uploading 872 frames at 13 MB each would take
//! hours and buy nothing — sharpness, exposure and duplicate detection are all
//! better done locally. What a vision model *is* genuinely better at than a
//! photometric heuristic is looking at a small, soft, half-turned face and
//! saying whether the eyes are shut.
//!
//! So we send only face crops, only for frames the local heuristic could not
//! confidently classify, and only when the user turns it on. A typical 872-frame
//! shoot produces a few dozen requests of a few kilobytes each, which sits
//! inside every free tier worth using.

use crate::decode::{crop_rgb, encode_jpeg, resize_rgb, Rgb8};
use crate::model::{Analysis, EyeSource, EyeState};
use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Crops outside this openness band are left to the local heuristic.
const BORDERLINE_LO: f32 = 0.34;
const BORDERLINE_HI: f32 = 0.66;
/// Long edge of the crop we upload. Enough to see an eyelid, small enough to
/// keep each request a few KB.
const CROP_SIZE: u32 = 192;
/// Most faces we will ask about in a single request.
const MAX_FACES_PER_REQUEST: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    /// Anthropic Messages API.
    Anthropic,
    /// Google Gemini. Has a free tier that comfortably covers this workload.
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConfig {
    pub enabled: bool,
    pub provider: Provider,
    pub api_key: String,
    /// Override the default model id. Empty means use the provider default.
    pub model: String,
    /// Hard ceiling on requests per run, so a misconfiguration cannot burn a
    /// quota or a bill.
    pub max_requests: usize,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: Provider::Gemini,
            api_key: String::new(),
            model: String::new(),
            max_requests: 120,
        }
    }
}

impl CloudConfig {
    fn model_id(&self) -> &str {
        if !self.model.is_empty() {
            return &self.model;
        }
        match self.provider {
            Provider::Anthropic => "claude-opus-5",
            Provider::Gemini => "gemini-2.0-flash",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudReport {
    pub requests: usize,
    pub faces_checked: usize,
    pub changed: usize,
    pub errors: Vec<String>,
    pub skipped_over_limit: usize,
}

/// One face we want a second opinion on.
struct Pending {
    item_index: usize,
    face_index: usize,
}

/// Which faces are ambiguous enough to be worth asking about.
fn borderline_faces(items: &[Analysis]) -> Vec<Pending> {
    let mut out = Vec::new();
    for (i, a) in items.iter().enumerate() {
        for (f, face) in a.faces.iter().enumerate() {
            // Only faces large enough for a useful crop.
            if face.area < 0.0015 {
                continue;
            }
            let ambiguous = face.eye_state == EyeState::Unknown
                || face.eye_state == EyeState::Squint
                || (face.eye_open > BORDERLINE_LO && face.eye_open < BORDERLINE_HI);
            if ambiguous {
                out.push(Pending {
                    item_index: i,
                    face_index: f,
                });
            }
        }
    }
    out
}

/// Run the verification pass, mutating eye states in place.
///
/// `load_image` decodes a photo at working resolution; it is passed in so this
/// module stays independent of the pipeline's caching.
pub fn verify<F>(items: &mut [Analysis], cfg: &CloudConfig, load_image: F) -> Result<CloudReport>
where
    F: Fn(&Analysis) -> Result<Rgb8>,
{
    let mut report = CloudReport {
        requests: 0,
        faces_checked: 0,
        changed: 0,
        errors: Vec::new(),
        skipped_over_limit: 0,
    };

    if !cfg.enabled {
        return Ok(report);
    }
    if cfg.api_key.trim().is_empty() {
        return Err(anyhow!("cloud verification is on but no API key was provided"));
    }

    let pending = borderline_faces(items);
    if pending.is_empty() {
        return Ok(report);
    }

    // Group by photo so one decode serves every face in the frame.
    let mut by_item: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for p in &pending {
        by_item.entry(p.item_index).or_default().push(p.face_index);
    }

    for (item_index, face_indices) in by_item {
        if report.requests >= cfg.max_requests {
            report.skipped_over_limit += face_indices.len();
            continue;
        }

        let img = match load_image(&items[item_index]) {
            Ok(i) => i,
            Err(e) => {
                report.errors.push(format!(
                    "{}: {e}",
                    items[item_index].meta.file_name
                ));
                continue;
            }
        };

        let take: Vec<usize> = face_indices.into_iter().take(MAX_FACES_PER_REQUEST).collect();
        let mut crops = Vec::new();
        for &fi in &take {
            let face = &items[item_index].faces[fi];
            // Widen the box a little; YuNet's boxes are tight and context helps.
            let cx = (face.x + face.w / 2.0) * img.w as f32;
            let cy = (face.y + face.h / 2.0) * img.h as f32;
            let side = (face.w * img.w as f32).max(face.h * img.h as f32) * 1.35;

            let crop = crop_rgb(
                &img,
                (cx - side / 2.0).round() as i32,
                (cy - side / 2.0).round() as i32,
                side.round().max(8.0) as u32,
                side.round().max(8.0) as u32,
            );
            let long = crop.w.max(crop.h).max(1);
            let scale = (CROP_SIZE as f32 / long as f32).min(1.0);
            let small = resize_rgb(
                &crop,
                ((crop.w as f32 * scale).round() as u32).max(1),
                ((crop.h as f32 * scale).round() as u32).max(1),
            );
            match encode_jpeg(&small, 85) {
                Ok(bytes) => crops.push(bytes),
                Err(e) => report.errors.push(format!("crop encode: {e}")),
            }
        }

        if crops.is_empty() {
            continue;
        }

        report.requests += 1;
        report.faces_checked += crops.len();

        let verdicts = match cfg.provider {
            Provider::Anthropic => ask_anthropic(cfg, &crops),
            Provider::Gemini => ask_gemini(cfg, &crops),
        };

        match verdicts {
            Ok(list) => {
                for (n, &fi) in take.iter().enumerate() {
                    if let Some(v) = list.get(n) {
                        let face = &mut items[item_index].faces[fi];
                        let new_state = match v.eyes.as_str() {
                            "open" => EyeState::Open,
                            "closed" => EyeState::Closed,
                            "squint" => EyeState::Squint,
                            _ => EyeState::Unknown,
                        };
                        if new_state != EyeState::Unknown {
                            if new_state != face.eye_state {
                                report.changed += 1;
                            }
                            face.eye_state = new_state;
                            face.eye_source = EyeSource::Cloud;
                            face.eye_open = match new_state {
                                EyeState::Open => 0.85,
                                EyeState::Squint => 0.45,
                                EyeState::Closed => 0.1,
                                EyeState::Unknown => face.eye_open,
                            };
                        }
                    }
                }
            }
            Err(e) => report
                .errors
                .push(format!("{}: {e}", items[item_index].meta.file_name)),
        }
    }

    Ok(report)
}

#[derive(Debug, Deserialize)]
struct Verdict {
    #[serde(default)]
    eyes: String,
}

const PROMPT: &str = "Each image is a crop of one person's face from a sports photograph. \
For each image in order, decide whether that person's eyes are open, closed, or squinting. \
\"closed\" means both eyes are shut, as in a blink. \"squint\" means partly closed but still \
showing iris. If the face is too blurry, too small, or turned too far away to tell, use \"unknown\". \
Reply with only a JSON array, one object per image, in the same order, like: \
[{\"eyes\":\"open\"},{\"eyes\":\"closed\"}]. No other text.";

fn ask_anthropic(cfg: &CloudConfig, crops: &[Vec<u8>]) -> Result<Vec<Verdict>> {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut content = Vec::new();
    for c in crops {
        content.push(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/jpeg",
                "data": engine.encode(c),
            }
        }));
    }
    content.push(serde_json::json!({ "type": "text", "text": PROMPT }));

    let body = serde_json::json!({
        "model": cfg.model_id(),
        "max_tokens": 256,
        // A one-line classification does not need deep reasoning; low effort
        // keeps this fast and cheap.
        "output_config": { "effort": "low" },
        // Route around a safety refusal rather than failing the whole pass.
        "betas": ["server-side-fallback-2026-07-01"],
        "fallbacks": "default",
        "messages": [{ "role": "user", "content": content }],
    });

    let resp = ureq::post("https://api.anthropic.com/v1/messages")
        .set("content-type", "application/json")
        .set("x-api-key", cfg.api_key.trim())
        .set("anthropic-version", "2023-06-01")
        .send_json(body)
        .map_err(describe_ureq)?;

    let json: serde_json::Value = resp.into_json().context("decode Anthropic response")?;

    if json.get("stop_reason").and_then(|v| v.as_str()) == Some("refusal") {
        return Err(anyhow!("model declined to classify this crop"));
    }

    let text = json
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    parse_verdicts(&text)
}

fn ask_gemini(cfg: &CloudConfig, crops: &[Vec<u8>]) -> Result<Vec<Verdict>> {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut parts = Vec::new();
    for c in crops {
        parts.push(serde_json::json!({
            "inline_data": { "mime_type": "image/jpeg", "data": engine.encode(c) }
        }));
    }
    parts.push(serde_json::json!({ "text": PROMPT }));

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        cfg.model_id()
    );
    let body = serde_json::json!({
        "contents": [{ "parts": parts }],
        "generationConfig": { "temperature": 0, "maxOutputTokens": 256 },
    });

    let resp = ureq::post(&url)
        .set("content-type", "application/json")
        .set("x-goog-api-key", cfg.api_key.trim())
        .send_json(body)
        .map_err(describe_ureq)?;

    let json: serde_json::Value = resp.into_json().context("decode Gemini response")?;
    let text = json
        .pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    parse_verdicts(&text)
}

/// Pull the JSON array out of a reply, tolerating markdown fences and stray prose.
fn parse_verdicts(text: &str) -> Result<Vec<Verdict>> {
    let start = text.find('[');
    let end = text.rfind(']');
    let slice = match (start, end) {
        (Some(s), Some(e)) if e > s => &text[s..=e],
        _ => return Err(anyhow!("no JSON array in reply: {}", truncate(text, 160))),
    };
    serde_json::from_str::<Vec<Verdict>>(slice)
        .with_context(|| format!("parse verdicts from {}", truncate(slice, 160)))
}

/// ureq folds HTTP errors into a variant that hides the response body, which is
/// where providers put the actual reason. Dig it out.
fn describe_ureq(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            anyhow!("HTTP {code}: {}", truncate(body.trim(), 300))
        }
        other => anyhow!(other),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}
