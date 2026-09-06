// Downloads the face-detection model into src-tauri/models.
//
// YuNet ships with the OpenCV Zoo under Apache-2.0 and is ~227 KB, so it is
// committed to the repo as well; this script exists so a fresh clone can
// re-fetch it if the file is missing or corrupt.

import { createWriteStream } from "node:fs";
import { mkdir, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const here = dirname(fileURLToPath(import.meta.url));
const modelDir = join(here, "..", "src-tauri", "models");

const MODELS = [
  {
    name: "face_detection_yunet_2023mar.onnx",
    url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
    minBytes: 200_000,
  },
  {
    // Subject detection. YOLOX-S rather than NanoDet-Plus: tract cannot load
    // the NanoDet export (its Resize uses pytorch_half_pixel), and YOLOX is
    // both more accurate (40.5 vs 34.1 mAP) and more confident in practice.
    name: "object_detection_yolox_2022nov.onnx",
    url: "https://github.com/opencv/opencv_zoo/raw/main/models/object_detection_yolox/object_detection_yolox_2022nov.onnx",
    minBytes: 30_000_000,
  },
];

await mkdir(modelDir, { recursive: true });

for (const m of MODELS) {
  const dest = join(modelDir, m.name);
  try {
    const s = await stat(dest);
    if (s.size >= m.minBytes) {
      console.log(`ok      ${m.name} (${(s.size / 1024).toFixed(0)} KB)`);
      continue;
    }
    console.log(`refetch ${m.name} — too small (${s.size} bytes)`);
  } catch {
    console.log(`fetch   ${m.name}`);
  }

  const res = await fetch(m.url, { redirect: "follow" });
  if (!res.ok) {
    console.error(`FAILED  ${m.name}: HTTP ${res.status}`);
    process.exitCode = 1;
    continue;
  }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest));
  const s = await stat(dest);
  console.log(`done    ${m.name} (${(s.size / 1024).toFixed(0)} KB)`);
}
