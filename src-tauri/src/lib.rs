//! Culling — automatic photo culling for sports and event shooters.
//!
//! Copyright (C) 2026 BitSec01
//!
//! This program is free software: you can redistribute it and/or modify it
//! under the terms of the GNU General Public License as published by the Free
//! Software Foundation, either version 3 of the License, or (at your option)
//! any later version.
//!
//! This program is distributed in the hope that it will be useful, but WITHOUT
//! ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
//! FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for
//! more details.
//!
//! You should have received a copy of the GNU General Public License along
//! with this program. If not, see <https://www.gnu.org/licenses/>.
//!
//! The backend owns all pixel work and all state; the webview is a pure view
//! layer that renders `Analysis` records and sends commands back.

pub mod cache;
pub mod cloud;
pub mod commands;
pub mod decode;
pub mod export;
pub mod faces;
pub mod group;
pub mod hash;
pub mod metrics;
pub mod model;
pub mod pipeline;
pub mod scan;
pub mod score;
pub mod subjects;
pub mod update;

use cloud::CloudConfig;
use model::{Analysis, ScoreSettings};
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Manager;

#[derive(Default)]
pub struct AppState {
    pub folder: Mutex<Option<PathBuf>>,
    pub items: RwLock<Vec<Analysis>>,
    pub settings: Mutex<ScoreSettings>,
    pub cloud: Mutex<CloudConfig>,
    pub cancel: Arc<AtomicBool>,
    pub running: AtomicBool,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        // Images are served through a custom scheme rather than the asset
        // protocol. That keeps us from having to whitelist the user's entire
        // filesystem, and lets us generate previews on demand.
        .register_uri_scheme_protocol("photo", move |ctx, request| {
            serve_photo(ctx.app_handle(), request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::analyze_folder,
            commands::cancel_analysis,
            commands::get_items,
            commands::get_settings,
            commands::update_settings,
            commands::set_pick,
            commands::set_picks,
            commands::set_rating,
            commands::export_preview,
            commands::export_run,
            commands::export_csv,
            commands::get_cloud_config,
            commands::set_cloud_config,
            commands::cloud_verify,
            commands::cache_info,
            commands::purge_cache,
            commands::check_for_update,
            commands::app_version,
            commands::reveal_in_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running culling");
}

/// Serve `photo://localhost/thumb/<id>` and `photo://localhost/preview/<id>?size=N`.
///
/// Only ids present in the current analysis are servable, so this cannot be
/// used to read arbitrary files off disk.
fn serve_photo(
    app: &tauri::AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let uri = request.uri();
    let path = uri.path().trim_start_matches('/').to_string();
    let query = uri.query().unwrap_or("");

    let mut parts = path.splitn(2, '/');
    let kind = parts.next().unwrap_or("");
    let id = parts.next().unwrap_or("");

    let state = app.state::<AppState>();
    let meta = {
        let items = state.items.read();
        items
            .iter()
            .find(|a| a.meta.id == id)
            .map(|a| a.meta.clone())
    };

    let Some(meta) = meta else {
        return not_found("unknown photo id");
    };
    let folder = state.folder.lock().clone();

    let bytes = match kind {
        "thumb" => match folder
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no folder loaded"))
            .and_then(|f| pipeline::ensure_thumb(f, &meta))
            .and_then(|p| Ok(std::fs::read(p)?))
        {
            Ok(b) => b,
            Err(e) => return not_found(&format!("thumb failed: {e}")),
        },
        "preview" => {
            let size = parse_size(query).unwrap_or(1600);
            match pipeline::preview_jpeg(&PathBuf::from(&meta.path), size, meta.orientation) {
                Ok(b) => b,
                Err(e) => return not_found(&format!("preview failed: {e}")),
            }
        }
        _ => return not_found("unknown resource"),
    };

    tauri::http::Response::builder()
        .status(200)
        .header("content-type", "image/jpeg")
        // Thumbnails are immutable for a given id+mtime, so let the webview
        // keep them. Without this, scrolling the grid re-reads from disk.
        .header("cache-control", "public, max-age=86400")
        .header("access-control-allow-origin", "*")
        .body(bytes)
        .unwrap_or_else(|_| not_found("response build failed"))
}

fn parse_size(query: &str) -> Option<u32> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == "size" {
            v.parse::<u32>().ok().map(|n| n.clamp(256, 6000))
        } else {
            None
        }
    })
}

fn not_found(msg: &str) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(404)
        .header("content-type", "text/plain")
        .body(msg.as_bytes().to_vec())
        .unwrap_or_default()
}

/// Convenience for tests and the CLI smoke check.
pub fn is_cancelled(state: &AppState) -> bool {
    state.cancel.load(Ordering::Relaxed)
}
