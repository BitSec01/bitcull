//! Reading RAW files.
//!
//! Culling does not need demosaiced sensor data, and decoding it would be slow
//! and would not look like the photograph anyway. Every RAW format embeds one
//! or more JPEG previews rendered by the camera itself — the same image the
//! back of the camera showed — and that is exactly what you want to judge a
//! frame by. Photo Mechanic and Lightroom's "embedded preview" mode work the
//! same way, which is why they feel instant.
//!
//! Rather than implement a parser per container (TIFF for CR2/NEF/ARW/DNG,
//! ISO-BMFF for CR3, a header offset for RAF), this scans for JPEG streams and
//! validates each candidate by parsing it. That is format-agnostic, so it also
//! works for formats not thought about here, and the validation step is what
//! keeps it honest: a run of `FF D8 FF` inside sensor data almost never parses
//! as a real JPEG with sane dimensions.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Previews below this are thumbnails, not something to judge focus on.
const MIN_EDGE: u16 = 320;

/// A JPEG stream found inside a container.
#[derive(Debug, Clone, Copy)]
pub struct Preview {
    pub offset: usize,
    pub len: usize,
    pub width: u16,
    pub height: u16,
}

impl Preview {
    fn pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Extract the largest usable JPEG preview from a RAW file.
pub fn extract_preview(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    // Safety: we only read, and a RAW file being rewritten underneath a cull
    // would be pathological. A torn read shows up as a failed JPEG parse, which
    // is handled, rather than as unsoundness in practice.
    let map = unsafe { memmap2::Mmap::map(&file)? };
    let best = find_best_preview(&map)
        .ok_or_else(|| anyhow!("no embedded JPEG preview found in {}", path.display()))?;
    Ok(map[best.offset..best.offset + best.len].to_vec())
}

/// Scan a buffer for embedded JPEGs and return the largest by pixel count.
pub fn find_best_preview(buf: &[u8]) -> Option<Preview> {
    let mut best: Option<Preview> = None;
    let mut from = 0usize;

    while let Some(rel) = memchr::memmem::find(&buf[from..], &[0xFF, 0xD8, 0xFF]) {
        let start = from + rel;
        if let Some(p) = parse_jpeg(buf, start) {
            if p.width >= MIN_EDGE
                && p.height >= MIN_EDGE
                && best.map_or(true, |b| p.pixels() > b.pixels())
            {
                best = Some(p);
            }
            // Skip past this stream; previews are not nested.
            from = start + p.len.max(3);
        } else {
            from = start + 3;
        }
        if from >= buf.len() {
            break;
        }
    }
    best
}

/// Walk JPEG markers from `start`, returning its extent and dimensions.
///
/// Returns `None` for anything that does not parse cleanly, which is how false
/// positives inside sensor data are rejected.
fn parse_jpeg(buf: &[u8], start: usize) -> Option<Preview> {
    let mut i = start + 2; // past SOI
    let mut dims: Option<(u16, u16)> = None;

    loop {
        // Markers may be preceded by fill bytes.
        while i < buf.len() && buf[i] == 0xFF && buf.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if i + 1 >= buf.len() || buf[i] != 0xFF {
            return None;
        }
        let marker = buf[i + 1];
        i += 2;

        match marker {
            // Standalone markers carry no payload.
            0x01 | 0xD0..=0xD7 => continue,
            0xD9 => return None, // EOI before SOS: not a real image
            0xDA => {
                // Start of scan: skip its header, then hunt for EOI. Inside
                // entropy-coded data an FF is always followed by 00 (stuffing)
                // or a restart marker, so a bare FF D9 is the true end.
                if i + 1 >= buf.len() {
                    return None;
                }
                let len = u16::from_be_bytes([buf[i], buf[i + 1]]) as usize;
                i += len;
                let (w, h) = dims?;
                let mut j = i;
                while j + 1 < buf.len() {
                    if buf[j] == 0xFF {
                        let n = buf[j + 1];
                        if n == 0xD9 {
                            return Some(Preview {
                                offset: start,
                                len: j + 2 - start,
                                width: w,
                                height: h,
                            });
                        }
                        if n == 0x00 || (0xD0..=0xD7).contains(&n) {
                            j += 2;
                            continue;
                        }
                    }
                    j += 1;
                }
                return None; // ran off the end without EOI
            }
            _ => {
                if i + 1 >= buf.len() {
                    return None;
                }
                let len = u16::from_be_bytes([buf[i], buf[i + 1]]) as usize;
                if len < 2 || i + len > buf.len() {
                    return None;
                }
                // Any Start-Of-Frame carries the dimensions. DAC (0xCC) and
                // DHT (0xC4) share the 0xCn range but are not frames.
                let is_sof = matches!(marker, 0xC0..=0xCF) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
                if is_sof && len >= 7 {
                    let h = u16::from_be_bytes([buf[i + 3], buf[i + 4]]);
                    let w = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
                    if w == 0 || h == 0 {
                        return None;
                    }
                    dims = Some((w, h));
                }
                i += len;
            }
        }
        if i >= buf.len() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal but structurally valid JPEG: SOI, SOF0, SOS, EOI.
    fn fake_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF]; // SOI + first byte of next marker
        v.pop();
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]); // SOF0, len 17
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[0x03]); // components
        v.extend_from_slice(&[0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        v.extend_from_slice(&[0x12, 0x34, 0xFF, 0x00, 0x56]); // entropy, with stuffing
        v.extend_from_slice(&[0xFF, 0xD9]); // EOI
        v
    }

    #[test]
    fn finds_a_single_preview() {
        let jpg = fake_jpeg(1024, 768);
        let mut buf = vec![0u8; 500];
        let at = buf.len();
        buf.extend_from_slice(&jpg);
        buf.extend_from_slice(&[0u8; 300]);

        let p = find_best_preview(&buf).expect("should find the preview");
        assert_eq!(p.offset, at);
        assert_eq!((p.width, p.height), (1024, 768));
        assert_eq!(&buf[p.offset..p.offset + p.len], jpg.as_slice());
    }

    /// RAW files hold a small thumbnail *and* a large preview; take the large one.
    #[test]
    fn prefers_the_largest_preview() {
        let small = fake_jpeg(640, 480);
        let large = fake_jpeg(2048, 1365);
        let mut buf = vec![0u8; 64];
        buf.extend_from_slice(&small);
        buf.extend_from_slice(&[0u8; 128]);
        buf.extend_from_slice(&large);

        let p = find_best_preview(&buf).unwrap();
        assert_eq!((p.width, p.height), (2048, 1365));
    }

    /// Thumbnails are not worth judging focus on.
    #[test]
    fn ignores_tiny_thumbnails() {
        let mut buf = vec![0u8; 32];
        buf.extend_from_slice(&fake_jpeg(160, 120));
        assert!(find_best_preview(&buf).is_none());
    }

    /// This is the case that makes the whole approach viable: sensor data is
    /// full of byte sequences that look like an SOI but do not parse.
    #[test]
    fn rejects_false_positives_in_sensor_data() {
        let mut buf = Vec::new();
        for i in 0..4000u32 {
            buf.extend_from_slice(&[0xFF, 0xD8, 0xFF, (i % 251) as u8, 0x7A, 0x03]);
        }
        assert!(find_best_preview(&buf).is_none());
    }

    /// A real preview must still be found in among that noise.
    #[test]
    fn finds_a_preview_surrounded_by_noise() {
        let mut buf = Vec::new();
        for i in 0..900u32 {
            buf.extend_from_slice(&[0xFF, 0xD8, 0xFF, (i % 251) as u8, 0x11]);
        }
        let at = buf.len();
        buf.extend_from_slice(&fake_jpeg(1600, 1067));
        for i in 0..900u32 {
            buf.extend_from_slice(&[0xFF, 0xD8, 0xFF, (i % 251) as u8, 0x11]);
        }
        let p = find_best_preview(&buf).unwrap();
        assert_eq!(p.offset, at);
        assert_eq!((p.width, p.height), (1600, 1067));
    }

    #[test]
    fn empty_and_truncated_input_is_safe() {
        assert!(find_best_preview(&[]).is_none());
        assert!(find_best_preview(&[0xFF, 0xD8, 0xFF]).is_none());
        let jpg = fake_jpeg(1024, 768);
        assert!(find_best_preview(&jpg[..jpg.len() - 4]).is_none());
    }
}
