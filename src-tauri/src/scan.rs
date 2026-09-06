//! Directory walking and EXIF extraction. Cheap enough to run on the whole
//! folder up front, so the grid can populate before any pixels are decoded.

use crate::model::PhotoMeta;
use anyhow::{Context, Result};
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

/// Proxy formats we analyse directly.
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// RAW formats we resolve to on export. Extensions only — we never decode these.
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", // Canon
    "nef", "nrw", // Nikon
    "arw", "srf", "sr2", // Sony
    "raf", // Fujifilm
    "orf", // Olympus / OM
    "rw2", // Panasonic
    "pef", "ptx",  // Pentax
    "dng",  // Adobe / DJI / Leica
    "3fr",  // Hasselblad
    "iiq",  // Phase One
    "gpr",  // GoPro
    "rwl",  // Leica
];

pub fn is_image(path: &Path) -> bool {
    ext_lower(path).map_or(false, |e| IMAGE_EXTS.contains(&e.as_str()))
}

pub fn is_raw(path: &Path) -> bool {
    ext_lower(path).map_or(false, |e| RAW_EXTS.contains(&e.as_str()))
}

pub fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Deterministic id for a path, so cached results survive restarts.
pub fn photo_id(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Collect every image under `root`. `recursive` follows subfolders.
pub fn scan_folder(root: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let walker = WalkDir::new(root)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .follow_links(false);

    let mut out: Vec<PathBuf> = walker
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| is_image(p))
        .collect();

    // Sort by name so bursts stay adjacent even when EXIF times are missing.
    out.sort_by(|a, b| {
        natural_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
    Ok(out)
}

/// Compare file names so `IMG_9.jpg` sorts before `IMG_10.jpg`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut na = String::new();
                    let mut nb = String::new();
                    while ai.peek().map_or(false, |c| c.is_ascii_digit()) {
                        na.push(ai.next().unwrap());
                    }
                    while bi.peek().map_or(false, |c| c.is_ascii_digit()) {
                        nb.push(bi.next().unwrap());
                    }
                    let va: u128 = na.trim_start_matches('0').parse().unwrap_or(0);
                    let vb: u128 = nb.trim_start_matches('0').parse().unwrap_or(0);
                    match va.cmp(&vb) {
                        std::cmp::Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    let (lx, ly) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
                    match lx.cmp(&ly) {
                        std::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        ord => return ord,
                    }
                }
            }
        }
    }
}

/// Read file stats + EXIF into a `PhotoMeta`. Never fails on bad EXIF; a photo
/// with unreadable metadata still gets analysed, it just loses burst grouping
/// by time and falls back to filename order.
pub fn read_meta(path: &Path) -> Result<PhotoMeta> {
    let md = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut meta = PhotoMeta {
        id: photo_id(path),
        path: path.to_string_lossy().to_string(),
        file_name: path.file_name().unwrap_or_default().to_string_lossy().to_string(),
        stem: path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
        ext: ext_lower(path).unwrap_or_default(),
        bytes: md.len(),
        mtime,
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
    };

    if let Ok(file) = File::open(path) {
        let mut reader = BufReader::new(file);
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
            fill_from_exif(&mut meta, &exif);
        }
    }

    // EXIF often omits dimensions on edited files; fall back to the JPEG header.
    if meta.width == 0 || meta.height == 0 {
        if let Ok((w, h)) = crate::decode::probe_dimensions(path) {
            meta.width = w;
            meta.height = h;
        }
    }

    Ok(meta)
}

fn fill_from_exif(meta: &mut PhotoMeta, exif: &exif::Exif) {
    use exif::{In, Tag, Value};

    let get = |tag: Tag| exif.get_field(tag, In::PRIMARY);

    if let Some(f) = get(Tag::PixelXDimension) {
        meta.width = f.value.get_uint(0).unwrap_or(0);
    }
    if let Some(f) = get(Tag::PixelYDimension) {
        meta.height = f.value.get_uint(0).unwrap_or(0);
    }
    if let Some(f) = get(Tag::Orientation) {
        meta.orientation = f.value.get_uint(0).unwrap_or(1) as u16;
    }
    if let Some(f) = get(Tag::Make) {
        let make = display(f);
        let model = get(Tag::Model).map(|m| display(m)).unwrap_or_default();
        // Canon writes "Canon" in Make and "Canon EOS R5" in Model; don't repeat it.
        meta.camera = Some(if model.starts_with(&make) {
            model
        } else {
            format!("{make} {model}").trim().to_string()
        });
    }
    if let Some(f) = get(Tag::LensModel) {
        meta.lens = Some(display(f));
    }
    if let Some(f) = get(Tag::PhotographicSensitivity) {
        meta.iso = f.value.get_uint(0);
    }
    if let Some(f) = get(Tag::FNumber) {
        meta.aperture = rational_f32(&f.value);
    }
    if let Some(f) = get(Tag::FocalLength) {
        meta.focal_len = rational_f32(&f.value);
    }
    if let Some(f) = get(Tag::ExposureTime) {
        meta.shutter = Some(display(f));
        meta.shutter_secs = rational_f32(&f.value);
    }
    if let Some(f) = get(Tag::SubSecTimeOriginal) {
        // Stored as an ASCII string like "42"; normalise to milliseconds.
        let s = display(f);
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let scaled = format!("{:0<3}", &digits[..digits.len().min(3)]);
            meta.sub_sec = scaled.parse::<u32>().ok();
        }
    }

    // Prefer DateTimeOriginal, then DateTimeDigitized, then DateTime.
    for tag in [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime] {
        if meta.taken_at.is_some() {
            break;
        }
        if let Some(f) = get(tag) {
            if let Value::Ascii(ref v) = f.value {
                if let Some(bytes) = v.first() {
                    if let Ok(dt) = exif::DateTime::from_ascii(bytes) {
                        meta.taken_at = Some(naive_to_unix(&dt));
                    }
                }
            }
        }
    }
}

fn display(f: &exif::Field) -> String {
    f.display_value().with_unit(()).to_string().trim().to_string()
}

fn rational_f32(v: &exif::Value) -> Option<f32> {
    match v {
        exif::Value::Rational(r) => r.first().map(|x| x.to_f32()),
        exif::Value::SRational(r) => r.first().map(|x| x.to_f32()),
        _ => None,
    }
}

/// EXIF timestamps carry no zone. We treat them as UTC, which is fine because
/// they are only ever compared against each other within one shoot.
fn naive_to_unix(dt: &exif::DateTime) -> i64 {
    let (y, m, d) = (dt.year as i64, dt.month as i64, dt.day as i64);
    // Days from civil, Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + dt.hour as i64 * 3600 + dt.minute as i64 * 60 + dt.second as i64
}

/// Index a RAW folder by lowercase stem, so proxies can be matched on export.
pub fn index_raws(root: &Path, recursive: bool) -> Result<std::collections::HashMap<String, PathBuf>> {
    let walker = WalkDir::new(root)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .follow_links(false);

    let mut map = std::collections::HashMap::new();
    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if !is_raw(&path) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            // First match wins; a shoot shouldn't have duplicate stems, and if
            // it does the shallower path is the more likely intent.
            map.entry(stem.to_ascii_lowercase()).or_insert(path);
        }
    }
    Ok(map)
}
