//! Perceptual hashing for near-duplicate detection.
//!
//! Two hashes are computed because they fail in different ways. dHash tracks
//! local gradient direction and is good at catching "same frame, tiny shift".
//! pHash works on the low-frequency DCT and tolerates exposure and colour
//! changes better. Burst frames from a sports camera are near-identical, so
//! agreement between the two is a strong duplicate signal.

use crate::decode::{resize_gray, Gray};

/// Difference hash: compare each pixel to its right neighbour on a 9x8 grid.
pub fn dhash(gray: &Gray) -> u64 {
    let small = resize_gray(gray, 9, 8);
    let mut bits = 0u64;
    let mut i = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            let left = small.at(x, y);
            let right = small.at(x + 1, y);
            if left > right {
                bits |= 1 << i;
            }
            i += 1;
        }
    }
    bits
}

/// Perceptual hash: 32x32 grayscale, 2D DCT, keep the top-left 8x8 (minus DC),
/// threshold against the median.
pub fn phash(gray: &Gray) -> u64 {
    const N: usize = 32;
    let small = resize_gray(gray, N as u32, N as u32);

    // Separable DCT-II. N is small enough that the naive O(N^3) form is fine.
    let mut rows = vec![0f32; N * N];
    let cos_table = dct_table(N);

    for y in 0..N {
        for u in 0..N {
            let mut sum = 0f32;
            for x in 0..N {
                sum += small.data[y * N + x] * cos_table[u * N + x];
            }
            rows[y * N + u] = sum;
        }
    }
    let mut coeffs = vec![0f32; N * N];
    for x in 0..N {
        for v in 0..N {
            let mut sum = 0f32;
            for y in 0..N {
                sum += rows[y * N + x] * cos_table[v * N + y];
            }
            coeffs[v * N + x] = sum;
        }
    }

    // Top-left 8x8 holds the low frequencies. Skip [0][0] (average brightness)
    // so the hash ignores overall exposure.
    let mut vals = Vec::with_capacity(64);
    for v in 0..8 {
        for u in 0..8 {
            vals.push(coeffs[v * N + u]);
        }
    }
    let mut sorted: Vec<f32> = vals.iter().skip(1).cloned().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    let mut bits = 0u64;
    for (i, v) in vals.iter().enumerate() {
        if *v > median {
            bits |= 1 << i;
        }
    }
    bits
}

fn dct_table(n: usize) -> Vec<f32> {
    let mut t = vec![0f32; n * n];
    for u in 0..n {
        for x in 0..n {
            t[u * n + x] =
                ((2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / (2.0 * n as f32)).cos();
        }
    }
    t
}

/// Coarse 8x8 grayscale signature, quantised to bytes.
///
/// Hashes are 1-bit-per-cell and saturate: two genuinely different frames can
/// land within a few bits of each other. Comparing signatures gives a
/// continuous second opinion that rescues those cases.
pub fn signature(gray: &Gray) -> Vec<u8> {
    let small = resize_gray(gray, 8, 8);
    small
        .data
        .iter()
        .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Mean absolute difference between two signatures, normalised to 0..1.
pub fn signature_distance(a: &[u8], b: &[u8]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 1.0;
    }
    let sum: u32 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs())
        .sum();
    (sum as f32 / a.len() as f32) / 255.0
}
