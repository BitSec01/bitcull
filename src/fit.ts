// Mapping normalised detection boxes onto a rendered <img>.
//
// Detections are normalised against the *photograph*. The element showing it
// is rarely the same shape, so a box drawn at the raw coordinates lands in the
// wrong place on anything that isn't exactly 3:2. These helpers convert once,
// and every overlay uses them.

import type { PhotoMeta } from "./types";

export interface Fit {
  /** Percentages, relative to the container. */
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The photo's on-screen aspect ratio, accounting for EXIF rotation. */
export function orientedAspect(meta: PhotoMeta): number {
  const swapped = meta.orientation >= 5 && meta.orientation <= 8;
  const w = swapped ? meta.height : meta.width;
  const h = swapped ? meta.width : meta.height;
  if (!w || !h) return 1.5;
  return w / h;
}

/**
 * `object-fit: contain` — the whole photo is visible, letterboxed or
 * pillarboxed inside a container of `containerAspect`.
 */
export function containFit(meta: PhotoMeta, containerAspect: number): Fit {
  const a = orientedAspect(meta);
  if (a > containerAspect) {
    const height = (containerAspect / a) * 100;
    return { left: 0, top: (100 - height) / 2, width: 100, height };
  }
  const width = (a / containerAspect) * 100;
  return { left: (100 - width) / 2, top: 0, width, height: 100 };
}

/**
 * `object-fit: cover` — the container is filled and the photo is cropped, so
 * the drawn area is *larger* than the container and offsets are negative.
 */
export function coverFit(meta: PhotoMeta, containerAspect: number): Fit {
  const a = orientedAspect(meta);
  if (a > containerAspect) {
    // Fills height, cropped left and right.
    const width = (a / containerAspect) * 100;
    return { left: (100 - width) / 2, top: 0, width, height: 100 };
  }
  const height = (containerAspect / a) * 100;
  return { left: 0, top: (100 - height) / 2, width: 100, height };
}

/** Place a normalised box (0..1) inside a fitted image. */
export function place(
  fit: Fit,
  box: { x: number; y: number; w: number; h: number },
) {
  return {
    left: `${fit.left + box.x * fit.width}%`,
    top: `${fit.top + box.y * fit.height}%`,
    width: `${box.w * fit.width}%`,
    height: `${box.h * fit.height}%`,
  };
}
