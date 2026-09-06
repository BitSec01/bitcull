//! Tauri command surface. Everything the UI can ask the backend to do.

use crate::cloud::{CloudConfig, CloudReport};
use crate::export::{ExportReport, ExportRequest, MatchReport};
use crate::faces::FaceDetector;
use crate::model::{Analysis, Pick, Progress, ScoreSettings};
use crate::pipeline::{self, Models, RunOptions};
use crate::subjects::SubjectDetector;
use crate::AppState;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Summary returned after a run, so the UI can show a headline without
/// recomputing anything.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub items: Vec<Analysis>,
    pub total: usize,
    pub keep: usize,
    pub maybe: usize,
    pub reject: usize,
    pub groups: usize,
    pub duration_ms: u64,
    /// Set when a requested model could not be loaded. Names which one.
    pub model_error: Option<String>,
}

fn summarise(items: Vec<Analysis>, duration_ms: u64, model_err: Option<String>) -> RunSummary {
    use crate::model::Verdict;
    let keep = items.iter().filter(|a| a.verdict == Verdict::Keep).count();
    let reject = items.iter().filter(|a| a.verdict == Verdict::Reject).count();
    let maybe = items.len() - keep - reject;
    let groups = items
        .iter()
        .filter_map(|a| a.group_id)
        .collect::<std::collections::HashSet<_>>()
        .len();

    RunSummary {
        total: items.len(),
        keep,
        maybe,
        reject,
        groups,
        duration_ms,
        model_error: model_err,
        items,
    }
}

#[tauri::command]
pub async fn analyze_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    folder: String,
    recursive: bool,
    detect_faces: bool,
    detect_subjects: bool,
    force: bool,
) -> Result<RunSummary, String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("An analysis is already running.".into());
    }
    state.cancel.store(false, Ordering::SeqCst);

    let folder_path = PathBuf::from(&folder);
    if !folder_path.is_dir() {
        state.running.store(false, Ordering::SeqCst);
        return Err(format!("Not a folder: {folder}"));
    }

    let settings = state.settings.lock().clone();
    let cancel = state.cancel.clone();

    // Load the models once per run and share them across worker threads.
    // A missing model is not fatal: everything it feeds degrades, the rest of
    // the analysis still runs, and the UI says exactly what was unavailable.
    let res_dir = app.path().resource_dir().ok();
    let mut model_errors: Vec<String> = Vec::new();
    let mut models = Models::default();

    if detect_faces {
        match FaceDetector::load(res_dir.clone()) {
            Ok(d) => models.faces = Some(Arc::new(d)),
            Err(e) => model_errors.push(format!("faces: {e}")),
        }
    }
    if detect_subjects {
        match SubjectDetector::load(res_dir.clone()) {
            Ok(d) => models.subjects = Some(Arc::new(d)),
            Err(e) => model_errors.push(format!("subjects: {e}")),
        }
    }
    let model_err = if model_errors.is_empty() {
        None
    } else {
        Some(model_errors.join("; "))
    };

    let opts = RunOptions {
        recursive,
        detect_faces: detect_faces && models.faces.is_some(),
        detect_subjects: detect_subjects && models.subjects.is_some(),
        force_reanalyse: force,
        settings,
    };

    let started = std::time::Instant::now();
    let emit_app = app.clone();
    let path_for_thread = folder_path.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        pipeline::run(&path_for_thread, &opts, models, cancel, move |p: Progress| {
            let _ = emit_app.emit("analysis:progress", p);
        })
    })
    .await;

    state.running.store(false, Ordering::SeqCst);

    let items = match result {
        Ok(Ok(items)) => items,
        Ok(Err(e)) => return Err(format!("Analysis failed: {e}")),
        Err(e) => return Err(format!("Analysis task panicked: {e}")),
    };

    *state.folder.lock() = Some(folder_path);
    *state.items.write() = items.clone();

    Ok(summarise(
        items,
        started.elapsed().as_millis() as u64,
        model_err,
    ))
}

#[tauri::command]
pub fn cancel_analysis(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn get_items(state: State<'_, AppState>) -> Vec<Analysis> {
    state.items.read().clone()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> ScoreSettings {
    state.settings.lock().clone()
}

/// Re-score with new settings. Cheap: no pixels are read, so the UI can call
/// this on every slider drag.
#[tauri::command]
pub fn update_settings(state: State<'_, AppState>, settings: ScoreSettings) -> RunSummary {
    *state.settings.lock() = settings.clone();
    let mut items = state.items.write();
    crate::score::rescore_all(&mut items, &settings);
    let out = items.clone();
    drop(items);
    summarise(out, 0, None)
}

#[tauri::command]
pub fn set_pick(state: State<'_, AppState>, id: String, pick: Option<Pick>) -> Option<Analysis> {
    let settings = state.settings.lock().clone();
    let mut items = state.items.write();
    let found = items.iter_mut().find(|a| a.meta.id == id).map(|a| {
        a.user_pick = pick;
        a.meta.id.clone()
    })?;

    // A manual pick can change which frame wins its group, so re-run the
    // group policy rather than patching one record.
    crate::score::rescore_all(&mut items, &settings);
    items.iter().find(|a| a.meta.id == found).cloned()
}

#[tauri::command]
pub fn set_rating(state: State<'_, AppState>, id: String, rating: u8) -> Option<Analysis> {
    let mut items = state.items.write();
    let a = items.iter_mut().find(|a| a.meta.id == id)?;
    a.rating = rating.min(5);
    Some(a.clone())
}

/// Bulk-apply a pick, for "reject all rejects" style actions.
#[tauri::command]
pub fn set_picks(state: State<'_, AppState>, ids: Vec<String>, pick: Option<Pick>) -> RunSummary {
    let settings = state.settings.lock().clone();
    let wanted: std::collections::HashSet<String> = ids.into_iter().collect();
    let mut items = state.items.write();
    for a in items.iter_mut() {
        if wanted.contains(&a.meta.id) {
            a.user_pick = pick;
        }
    }
    crate::score::rescore_all(&mut items, &settings);
    let out = items.clone();
    drop(items);
    summarise(out, 0, None)
}

#[tauri::command]
pub fn export_preview(
    state: State<'_, AppState>,
    request: ExportRequest,
) -> Result<MatchReport, String> {
    let items = state.items.read();
    crate::export::preview(&items, &request).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_run(
    state: State<'_, AppState>,
    request: ExportRequest,
) -> Result<ExportReport, String> {
    let items = state.items.read();
    crate::export::run(&items, &request).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn export_csv(state: State<'_, AppState>, dest: String) -> Result<(), String> {
    let items = state.items.read();
    crate::export::write_csv(&items, &PathBuf::from(dest)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_cloud_config(state: State<'_, AppState>) -> CloudConfig {
    let mut cfg = state.cloud.lock().clone();
    // Never hand the key back to the UI once stored.
    if !cfg.api_key.is_empty() {
        cfg.api_key = "••••••••".into();
    }
    cfg
}

#[tauri::command]
pub fn set_cloud_config(state: State<'_, AppState>, config: CloudConfig) {
    let mut cur = state.cloud.lock();
    // An unchanged masked key means "keep what you have".
    let key = if config.api_key.chars().all(|c| c == '•') && !config.api_key.is_empty() {
        cur.api_key.clone()
    } else {
        config.api_key.clone()
    };
    *cur = CloudConfig { api_key: key, ..config };
}

#[tauri::command]
pub async fn cloud_verify(state: State<'_, AppState>) -> Result<CloudReport, String> {
    let cfg = state.cloud.lock().clone();
    if !cfg.enabled {
        return Err("Cloud verification is turned off.".into());
    }
    let mut items = state.items.read().clone();
    let settings = state.settings.lock().clone();

    let report = tauri::async_runtime::spawn_blocking(move || {
        let out = crate::cloud::verify(&mut items, &cfg, |a| {
            let path = PathBuf::from(&a.meta.path);
            let rgb = crate::decode::decode_scaled(&path, pipeline::WORK_SIZE)?;
            Ok(crate::decode::apply_orientation(rgb, a.meta.orientation))
        });
        out.map(|r| (r, items))
    })
    .await
    .map_err(|e| format!("Cloud task panicked: {e}"))?
    .map_err(|e| e.to_string())?;

    let (report, mut items) = report;
    crate::score::rescore_all(&mut items, &settings);
    *state.items.write() = items;
    Ok(report)
}

/// Ask GitHub whether a newer release exists.
///
/// Returns `None` rather than an error when the check fails — being offline,
/// or behind a proxy, must never look like something is broken.
#[tauri::command]
pub async fn check_for_update() -> Option<crate::update::UpdateInfo> {
    tauri::async_runtime::spawn_blocking(|| crate::update::check().ok())
        .await
        .ok()
        .flatten()
}

#[tauri::command]
pub fn app_version() -> String {
    crate::update::current_version().to_string()
}

#[tauri::command]
pub fn cache_info() -> u64 {
    crate::cache::cache_size()
}

#[tauri::command]
pub fn purge_cache() -> Result<(), String> {
    crate::cache::purge_all().map_err(|e| e.to_string())
}

/// Reveal a file in Explorer / Finder / the Linux file manager.
#[tauri::command]
pub fn reveal_in_folder(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("No such file: {path}"));
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R"])
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No portable "select the file" call on Linux; open the parent folder.
        let dir = p.parent().unwrap_or(&p);
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

