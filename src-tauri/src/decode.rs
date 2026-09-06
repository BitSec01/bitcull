//! Image decoding.
//!
//! The whole pipeline hinges on this file being fast. A 45 MP Canon frame is
//! ~13 MB and there are hundreds of them, so we never decode at full size:
//! `jpeg-decoder` can scale during the IDCT (1/8, 1/4, 1/2 ...), which skips
//! most of the per-pixel work while still reading the entropy-coded stream.
//!
//! Analysis runs at ~2048 px on the long edge. That is small enough to be
//! quick and large enough that missed focus and motion smear are still
//! measurable — at 1024 px a soft frame and a tack-sharp one look identical.

use anyhow::{anyhow, Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Interleaved 8-bit RGB.
pub struct Rgb8 {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Single channel luminance in 0..1.
pub struct Gray {
    pub w: u32,
    pub h: u32,
    pub data: Vec<f32>,
}

impl Gray {
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> f32 {
        self.data[(y * self.w + x) as usize]
    }
}

/// Read image dimensions without decoding pixel data.
pub fn probe_dimensions(path: &Path) -> Result<(u32, u32)> {
    if crate::scan::is_raw(path) {
        // Report the preview's size: that is the image being judged, and the
        // sensor dimensions would make the detection boxes land wrong.
        let jpeg = crate::raw::extract_preview(path)?;
        let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(&jpeg));
        dec.read_info()?;
        let info = dec.info().ok_or_else(|| anyhow!("no jpeg info"))?;
        return Ok((info.width as u32, info.height as u32));
    }
    if is_jpeg(path) {
        let file = File::open(path)?;
        let mut dec = jpeg_decoder::Decoder::new(BufReader::new(file));
        dec.read_info()
            .with_context(|| format!("read jpeg header {}", path.display()))?;
        let info = dec.info().ok_or_else(|| anyhow!("no jpeg info"))?;
        Ok((info.width as u32, info.height as u32))
    } else {
        let reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let (w, h) = reader.into_dimensions()?;
        Ok((w, h))
    }
}

fn is_jpeg(path: &Path) -> bool {
    matches!(
        crate::scan::ext_lower(path).as_deref(),
        Some("jpg") | Some("jpeg")
    )
}

/// Decode `path` so that its long edge is close to `target_long_edge`.
///
/// The result may be larger than requested — DCT scaling only offers discrete
/// steps, and progressive JPEGs do not support it at all — so callers that
/// need an exact size must resample afterwards.
pub fn decode_scaled(path: &Path, target_long_edge: u32) -> Result<Rgb8> {
    if crate::scan::is_raw(path) {
        // RAW files are judged by their embedded camera-rendered preview.
        let jpeg = crate::raw::extract_preview(path)?;
        decode_jpeg_bytes(&jpeg, target_long_edge)
            .with_context(|| format!("decode embedded preview of {}", path.display()))
    } else if is_jpeg(path) {
        decode_jpeg_scaled(path, target_long_edge)
    } else {
        let img = image::ImageReader::open(path)?
            .with_guessed_format()?
            .decode()
            .with_context(|| format!("decode {}", path.display()))?;
        let (w, h) = (img.width(), img.height());
        let long = w.max(h);
        let img = if long > target_long_edge {
            let scale = target_long_edge as f32 / long as f32;
            img.resize(
                ((w as f32 * scale).round() as u32).max(1),
                ((h as f32 * scale).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };
        let rgb = img.to_rgb8();
        Ok(Rgb8 {
            w: rgb.width(),
            h: rgb.height(),
            data: rgb.into_raw(),
        })
    }
}

fn decode_jpeg_scaled(path: &Path, target_long_edge: u32) -> Result<Rgb8> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    decode_jpeg_reader(BufReader::new(file), target_long_edge)
}

/// Decode a JPEG already in memory — used for RAW embedded previews.
pub fn decode_jpeg_bytes(bytes: &[u8], target_long_edge: u32) -> Result<Rgb8> {
    decode_jpeg_reader(std::io::Cursor::new(bytes), target_long_edge)
}

fn decode_jpeg_reader<R: std::io::Read>(reader: R, target_long_edge: u32) -> Result<Rgb8> {
    let mut dec = jpeg_decoder::Decoder::new(reader);
    dec.read_info().context("read jpeg header")?;
    let info = dec.info().ok_or_else(|| anyhow!("no jpeg info"))?;

    let (fw, fh) = (info.width as u32, info.height as u32);
    let long = fw.max(fh).max(1);

    // Ask for the scaled size; the decoder rounds to the nearest eighth it can
    // actually produce and tells us what it picked.
    if long > target_long_edge {
        let scale = target_long_edge as f32 / long as f32;
        let rw = ((fw as f32 * scale).round() as u32).clamp(1, u16::MAX as u32) as u16;
        let rh = ((fh as f32 * scale).round() as u32).clamp(1, u16::MAX as u32) as u16;
        // A failure here is not fatal: we just decode at full size instead.
        let _ = dec.scale(rw, rh);
    }

    let pixels = dec
        .decode()
        .context("decode jpeg")?;
    let info = dec.info().ok_or_else(|| anyhow!("no jpeg info after decode"))?;
    let (w, h) = (info.width as u32, info.height as u32);

    let data = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels,
        jpeg_decoder::PixelFormat::L8 => {
            let mut out = Vec::with_capacity(pixels.len() * 3);
            for v in pixels {
                out.extend_from_slice(&[v, v, v]);
            }
            out
        }
        jpeg_decoder::PixelFormat::L16 => {
            let mut out = Vec::with_capacity((pixels.len() / 2) * 3);
            for chunk in pixels.chunks_exact(2) {
                let v = (u16::from_le_bytes([chunk[0], chunk[1]]) >> 8) as u8;
                out.extend_from_slice(&[v, v, v]);
            }
            out
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            // Adobe CMYK JPEGs store inverted values.
            let mut out = Vec::with_capacity((pixels.len() / 4) * 3);
            for px in pixels.chunks_exact(4) {
                let (c, m, y, k) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
                out.push(((c * k) / 255) as u8);
                out.push(((m * k) / 255) as u8);
                out.push(((y * k) / 255) as u8);
            }
            out
        }
    };

    if data.len() < (w as usize * h as usize * 3) {
        return Err(anyhow!(
            "short pixel buffer ({} bytes for {}x{})",
            data.len(),
            w,
            h
        ));
    }

    Ok(Rgb8 { w, h, data })
}

/// Rotate/flip so the image matches how a viewer would show it. Only pays the
/// copy when EXIF actually asks for a transform.
pub fn apply_orientation(img: Rgb8, orientation: u16) -> Rgb8 {
    if orientation <= 1 || orientation > 8 {
        return img;
    }
    let (w, h) = (img.w as usize, img.h as usize);
    let src = &img.data;
    // 5..8 transpose the axes.
    let swaps = matches!(orientation, 5 | 6 | 7 | 8);
    let (nw, nh) = if swaps { (h, w) } else { (w, h) };
    let mut out = vec![0u8; nw * nh * 3];

    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                8 => (y, w - 1 - x),
                _ => (x, y),
            };
            let si = (y * w + x) * 3;
            let di = (ny * nw + nx) * 3;
            out[di..di + 3].copy_from_slice(&src[si..si + 3]);
        }
    }

    Rgb8 {
        w: nw as u32,
        h: nh as u32,
        data: out,
    }
}

/// Rec. 709 luminance, normalised to 0..1.
pub fn to_gray(img: &Rgb8) -> Gray {
    let n = (img.w as usize) * (img.h as usize);
    let mut data = Vec::with_capacity(n);
    for px in img.data.chunks_exact(3).take(n) {
        let y = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        data.push(y / 255.0);
    }
    Gray {
        w: img.w,
        h: img.h,
        data,
    }
}

/// Box-filtered downscale of a grayscale image. Used for hashing, where we
/// want averaged energy rather than point samples.
pub fn resize_gray(src: &Gray, nw: u32, nh: u32) -> Gray {
    if nw == src.w && nh == src.h {
        return Gray {
            w: nw,
            h: nh,
            data: src.data.clone(),
        };
    }
    let mut data = vec![0f32; (nw * nh) as usize];
    let sx = src.w as f32 / nw as f32;
    let sy = src.h as f32 / nh as f32;

    for oy in 0..nh {
        let y0 = (oy as f32 * sy).floor() as u32;
        let y1 = (((oy + 1) as f32 * sy).ceil() as u32).min(src.h).max(y0 + 1);
        for ox in 0..nw {
            let x0 = (ox as f32 * sx).floor() as u32;
            let x1 = (((ox + 1) as f32 * sx).ceil() as u32).min(src.w).max(x0 + 1);
            let mut sum = 0f32;
            let mut count = 0f32;
            for y in y0..y1 {
                let row = (y * src.w) as usize;
                for x in x0..x1 {
                    sum += src.data[row + x as usize];
                    count += 1.0;
                }
            }
            data[(oy * nw + ox) as usize] = if count > 0.0 { sum / count } else { 0.0 };
        }
    }
    Gray { w: nw, h: nh, data }
}

/// Bilinear RGB resize, used for thumbnails and for feeding fixed-size model
/// inputs.
pub fn resize_rgb(src: &Rgb8, nw: u32, nh: u32) -> Rgb8 {
    if nw == src.w && nh == src.h {
        return Rgb8 {
            w: nw,
            h: nh,
            data: src.data.clone(),
        };
    }
    let mut data = vec![0u8; (nw as usize) * (nh as usize) * 3];
    let xr = if nw > 1 {
        (src.w.saturating_sub(1)) as f32 / (nw - 1) as f32
    } else {
        0.0
    };
    let yr = if nh > 1 {
        (src.h.saturating_sub(1)) as f32 / (nh - 1) as f32
    } else {
        0.0
    };

    for oy in 0..nh {
        let fy = oy as f32 * yr;
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(src.h - 1);
        let wy = fy - y0 as f32;
        for ox in 0..nw {
            let fx = ox as f32 * xr;
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(src.w - 1);
            let wx = fx - x0 as f32;

            let i00 = ((y0 * src.w + x0) * 3) as usize;
            let i01 = ((y0 * src.w + x1) * 3) as usize;
            let i10 = ((y1 * src.w + x0) * 3) as usize;
            let i11 = ((y1 * src.w + x1) * 3) as usize;
            let di = ((oy * nw + ox) * 3) as usize;

            for c in 0..3 {
                let top = src.data[i00 + c] as f32 * (1.0 - wx) + src.data[i01 + c] as f32 * wx;
                let bot = src.data[i10 + c] as f32 * (1.0 - wx) + src.data[i11 + c] as f32 * wx;
                data[di + c] = (top * (1.0 - wy) + bot * wy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Rgb8 { w: nw, h: nh, data }
}

/// Crop in pixel coordinates, clamped to the image bounds.
pub fn crop_rgb(src: &Rgb8, x: i32, y: i32, w: u32, h: u32) -> Rgb8 {
    let x0 = x.clamp(0, src.w as i32 - 1) as u32;
    let y0 = y.clamp(0, src.h as i32 - 1) as u32;
    let x1 = (x0 + w).min(src.w);
    let y1 = (y0 + h).min(src.h);
    let (cw, ch) = ((x1 - x0).max(1), (y1 - y0).max(1));

    let mut data = vec![0u8; (cw as usize) * (ch as usize) * 3];
    for row in 0..ch {
        let si = (((y0 + row) * src.w + x0) * 3) as usize;
        let di = (row * cw * 3) as usize;
        let len = (cw * 3) as usize;
        if si + len <= src.data.len() {
            data[di..di + len].copy_from_slice(&src.data[si..si + len]);
        }
    }
    Rgb8 { w: cw, h: ch, data }
}

/// Encode to JPEG bytes at the given quality.
pub fn encode_jpeg(img: &Rgb8, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    enc.encode(&img.data, img.w, img.h, image::ExtendedColorType::Rgb8)?;
    Ok(buf)
}
