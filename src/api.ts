// Thin typed wrappers over the Tauri command surface.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Analysis,
  CloudConfig,
  CloudReport,
  ExportReport,
  ExportRequest,
  MatchReport,
  Pick,
  Progress,
  RunSummary,
  ScoreSettings,
} from "./types";

/// Windows rewrites custom URI schemes to `http://<scheme>.localhost`.
const isWindows = navigator.userAgent.includes("Windows");
const photoBase = isWindows ? "http://photo.localhost" : "photo://localhost";

export const thumbUrl = (id: string) => `${photoBase}/thumb/${id}`;
export const previewUrl = (id: string, size = 1600) =>
  `${photoBase}/preview/${id}?size=${size}`;

export const analyzeFolder = (
  folder: string,
  recursive: boolean,
  detectFaces: boolean,
  detectSubjects: boolean,
  force: boolean,
) =>
  invoke<RunSummary>("analyze_folder", {
    folder,
    recursive,
    detectFaces,
    detectSubjects,
    force,
  });

export const cancelAnalysis = () => invoke<void>("cancel_analysis");
export const getItems = () => invoke<Analysis[]>("get_items");
export const getSettings = () => invoke<ScoreSettings>("get_settings");
export const updateSettings = (settings: ScoreSettings) =>
  invoke<RunSummary>("update_settings", { settings });

export const setPick = (id: string, pick: Pick | null) =>
  invoke<Analysis | null>("set_pick", { id, pick });
export const setPicks = (ids: string[], pick: Pick | null) =>
  invoke<RunSummary>("set_picks", { ids, pick });
export const setRating = (id: string, rating: number) =>
  invoke<Analysis | null>("set_rating", { id, rating });

export const exportPreview = (request: ExportRequest) =>
  invoke<MatchReport>("export_preview", { request });
export const exportRun = (request: ExportRequest) =>
  invoke<ExportReport>("export_run", { request });
export const exportCsv = (dest: string) => invoke<void>("export_csv", { dest });

export const getCloudConfig = () => invoke<CloudConfig>("get_cloud_config");
export const setCloudConfig = (config: CloudConfig) =>
  invoke<void>("set_cloud_config", { config });
export const cloudVerify = () => invoke<CloudReport>("cloud_verify");

export interface UpdateInfo {
  current: string;
  latest: string;
  newer: boolean;
  url: string;
  notes: string;
}

export const checkForUpdate = () =>
  invoke<UpdateInfo | null>("check_for_update");
export const appVersion = () => invoke<string>("app_version");

export const cacheInfo = () => invoke<number>("cache_info");
export const purgeCache = () => invoke<void>("purge_cache");
export const revealInFolder = (path: string) =>
  invoke<void>("reveal_in_folder", { path });

export const onProgress = (cb: (p: Progress) => void) =>
  listen<Progress>("analysis:progress", (e) => cb(e.payload));
