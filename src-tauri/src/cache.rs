//! On-disk caching of analysis results and thumbnails.
//!
//! Analysing 872 45 MP frames takes real time, and the user will reopen the
//! same folder repeatedly while reviewing. Results are keyed by path + mtime +
//! size, so an edited or replaced file is re-analysed automatically while
//! everything else loads instantly.

use crate::model::Analysis;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Bump when the meaning of any stored metric changes, so stale results are
/// discarded rather than silently mixed with new ones.
const CACHE_VERSION: u32 = 4;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CacheEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CacheEntry {
    mtime: i64,
    bytes: u64,
    analysis: Analysis,
}

pub struct Cache {
    root: PathBuf,
    folder_key: String,
    entries: HashMap<String, CacheEntry>,
}

impl Cache {
    /// Open (or create) the cache for one folder.
    pub fn open(folder: &Path) -> Result<Self> {
        let root = cache_root()?;
        std::fs::create_dir_all(root.join("thumbs"))?;
        std::fs::create_dir_all(root.join("analysis"))?;

        let folder_key = hash_str(&folder.to_string_lossy().to_lowercase());
        let path = root.join("analysis").join(format!("{folder_key}.json"));

        let entries = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<CacheFile>(&bytes) {
                Ok(cf) if cf.version == CACHE_VERSION => cf.entries,
                // A malformed or outdated cache is not an error worth surfacing;
                // it just means everything gets recomputed.
                _ => HashMap::new(),
            },
            Err(_) => HashMap::new(),
        };

        Ok(Self {
            root,
            folder_key,
            entries,
        })
    }

    /// Fetch a cached result if the file has not changed since it was stored.
    pub fn get(&self, id: &str, mtime: i64, bytes: u64) -> Option<Analysis> {
        self.entries.get(id).and_then(|e| {
            if e.mtime == mtime && e.bytes == bytes {
                Some(e.analysis.clone())
            } else {
                None
            }
        })
    }

    pub fn put(&mut self, analysis: &Analysis) {
        self.entries.insert(
            analysis.meta.id.clone(),
            CacheEntry {
                mtime: analysis.meta.mtime,
                bytes: analysis.meta.bytes,
                analysis: analysis.clone(),
            },
        );
    }

    pub fn save(&self) -> Result<()> {
        let path = self
            .root
            .join("analysis")
            .join(format!("{}.json", self.folder_key));
        let cf = CacheFile {
            version: CACHE_VERSION,
            entries: self.entries.clone(),
        };
        let bytes = serde_json::to_vec(&cf)?;
        // Write to a temp file then rename, so an interrupted save cannot
        // leave a truncated cache behind.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Forget every stored result for this folder.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn thumb_path(&self, id: &str, mtime: i64, size: u32) -> PathBuf {
        self.root
            .join("thumbs")
            .join(format!("{id}_{mtime}_{size}.jpg"))
    }
}

pub fn cache_root() -> Result<PathBuf> {
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    Ok(base.join("culling"))
}

/// Total bytes currently held in the cache, for the settings panel.
pub fn cache_size() -> u64 {
    let Ok(root) = cache_root() else { return 0 };
    let mut total = 0u64;
    for sub in ["thumbs", "analysis"] {
        if let Ok(rd) = std::fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                if let Ok(md) = e.metadata() {
                    total += md.len();
                }
            }
        }
    }
    total
}

/// Delete every cached thumbnail and result across all folders.
pub fn purge_all() -> Result<()> {
    let root = cache_root()?;
    for sub in ["thumbs", "analysis"] {
        let dir = root.join(sub);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
    }
    Ok(())
}

fn hash_str(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}
