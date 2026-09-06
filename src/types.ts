// Mirror of the Rust types in src-tauri/src/model.rs.
// Keep these in sync — serde uses camelCase on the wire.

export type Verdict = "keep" | "maybe" | "reject";
/** Whether a frame needs a subject at all. See `model.rs`. */
export type SubjectPolicy = "auto" | "require" | "scenery";
export type Severity = "good" | "warn" | "bad";
export type Pick = "keep" | "reject";
export type EyeState = "open" | "squint" | "closed" | "unknown";
export type EyeSource = "heuristic" | "model" | "cloud";

export interface PhotoMeta {
  id: string;
  path: string;
  fileName: string;
  stem: string;
  ext: string;
  bytes: number;
  mtime: number;
  width: number;
  height: number;
  orientation: number;
  takenAt: number | null;
  subSec: number | null;
  camera: string | null;
  lens: string | null;
  iso: number | null;
  shutter: string | null;
  shutterSecs: number | null;
  aperture: number | null;
  focalLen: number | null;
}

export interface ExposureStats {
  mean: number;
  stddev: number;
  clippedHighlights: number;
  clippedShadows: number;
  bias: number;
}

export interface Metrics {
  /** Raw Laplacian variance before log compression, for calibration checks. */
  sharpnessRaw: number;
  sharpnessGlobal: number;
  sharpnessPeak: number;
  sharpnessSubject: number;
  focusX: number;
  focusY: number;
  motionBlur: number;
  motionAngle: number;
  exposure: ExposureStats;
  noise: number;
  phash: number;
  dhash: number;
  signature: number[];
}

export interface Face {
  x: number;
  y: number;
  w: number;
  h: number;
  score: number;
  landmarks: [number, number][];
  eyeOpen: number;
  eyeState: EyeState;
  eyeSource: EyeSource;
  sharpness: number;
  area: number;
}

export interface Subject {
  classId: number;
  label: string;
  x: number;
  y: number;
  w: number;
  h: number;
  score: number;
  area: number;
  sharpness: number;
  counts: boolean;
}

export interface SubjectStats {
  mainArea: number;
  mainSharpness: number;
  backgroundSharpness: number;
  count: number;
  totalArea: number;
}

export interface Reason {
  code: string;
  label: string;
  severity: Severity;
  delta: number;
}

export interface Analysis {
  meta: PhotoMeta;
  metrics: Metrics;
  faces: Face[];
  subjects: Subject[];
  subjectStats: SubjectStats;
  score: number;
  verdict: Verdict;
  reasons: Reason[];
  groupId: number | null;
  isGroupBest: boolean;
  groupRank: number;
  userPick: Pick | null;
  rating: number;
  error: string | null;
}

export interface ScoreSettings {
  sharpnessFloor: number;
  keepThreshold: number;
  rejectThreshold: number;
  wSharpness: number;
  wExposure: number;
  wFaces: number;
  wComposition: number;
  wSubject: number;
  subjectPolicy: SubjectPolicy;
  subjectClasses: number[];
  subjectConfidence: number;
  burstGapSecs: number;
  groupByTime: boolean;
  groupByAppearance: boolean;
  hashDistance: number;
  keepOnePerGroup: boolean;
  strictEyes: boolean;
}

export interface Progress {
  done: number;
  total: number;
  current: string;
  stage: string;
  elapsedMs: number;
  etaMs: number | null;
}

export interface RunSummary {
  items: Analysis[];
  total: number;
  keep: number;
  maybe: number;
  reject: number;
  groups: number;
  durationMs: number;
  /** Set when a requested model could not be loaded. Names which one. */
  modelError: string | null;
  /** True when the set is being judged as scenery, not as subject frames. */
  scenery: boolean;
}

export type ExportAction = "copy" | "move" | "listOnly" | "sidecar";
export type ExportTarget = "raw" | "proxy";

export interface ExportRequest {
  action: ExportAction;
  target: ExportTarget;
  rawFolder: string | null;
  rawRecursive: boolean;
  destination: string | null;
  ids: string[];
  includeRejectsSubfolder: boolean;
}

export interface MatchReport {
  selected: number;
  matched: number;
  unmatched: string[];
  samples: [string, string][];
  rawExtensions: string[];
}

export interface ExportReport {
  matched: number;
  exported: number;
  skipped: number;
  unmatched: string[];
  errors: string[];
  destination: string | null;
}

export type CloudProvider = "anthropic" | "gemini";

export interface CloudConfig {
  enabled: boolean;
  provider: CloudProvider;
  apiKey: string;
  model: string;
  maxRequests: number;
}

export interface CloudReport {
  requests: number;
  facesChecked: number;
  changed: number;
  errors: string[];
  skippedOverLimit: number;
}
