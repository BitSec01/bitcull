//! Turning a cull into something you can act on.
//!
//! The JPEGs being reviewed are proxies. What the photographer actually needs
//! is the matching RAW files for the frames worth editing, so every export path
//! here resolves proxy -> RAW by file stem (`026A2371.JPG` -> `026A2371.CR3`)
//! and reports anything it could not match rather than silently dropping it.
//!
//! Nothing here deletes originals. The most destructive option is a move, and
//! even that refuses to overwrite an existing file.

use crate::model::{Analysis, Verdict};
use crate::scan;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportAction {
    /// Copy matched files to the destination.
    Copy,
    /// Move matched files to the destination. Never overwrites.
    Move,
    /// Write a plain text list of matched paths.
    ListOnly,
    /// Write XMP sidecars next to the originals carrying rating and label.
    Sidecar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportTarget {
    /// Act on the RAW files matching the selected proxies.
    Raw,
    /// Act on the proxy JPEGs themselves.
    Proxy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub action: ExportAction,
    pub target: ExportTarget,
    /// Folder holding the RAW files. Required when `target` is `Raw`.
    pub raw_folder: Option<String>,
    pub raw_recursive: bool,
    /// Destination folder for copy/move, or the file to write for `ListOnly`.
    pub destination: Option<String>,
    /// Ids of the photos to export.
    pub ids: Vec<String>,
    /// Also write the rejects into a `rejected` subfolder.
    pub include_rejects_subfolder: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub matched: usize,
    pub exported: usize,
    pub skipped: usize,
    /// Proxies with no corresponding RAW file.
    pub unmatched: Vec<String>,
    /// Per-file failures, as "name: reason".
    pub errors: Vec<String>,
    pub destination: Option<String>,
}

/// Preview what an export would touch, without writing anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchReport {
    pub selected: usize,
    pub matched: usize,
    pub unmatched: Vec<String>,
    /// Example resolved pairs, for the confirmation dialog.
    pub samples: Vec<[String; 2]>,
    /// Extensions found in the RAW folder, so the user can spot a wrong folder.
    pub raw_extensions: Vec<String>,
}

/// Resolve the selected proxies to the files an export would act on.
fn resolve(
    items: &[Analysis],
    req: &ExportRequest,
) -> Result<(Vec<(PathBuf, Analysis)>, Vec<String>, Vec<String>)> {
    let wanted: std::collections::HashSet<&str> = req.ids.iter().map(|s| s.as_str()).collect();
    let selected: Vec<&Analysis> = items
        .iter()
        .filter(|a| wanted.contains(a.meta.id.as_str()))
        .collect();

    let mut resolved = Vec::new();
    let mut unmatched = Vec::new();
    let mut raw_exts: std::collections::BTreeSet<String> = Default::default();

    match req.target {
        ExportTarget::Proxy => {
            for a in selected {
                resolved.push((PathBuf::from(&a.meta.path), (*a).clone()));
            }
        }
        // Culling a folder of RAW files directly: the "RAW" for each item is
        // the item itself, so there is nothing to match against.
        ExportTarget::Raw
            if selected
                .iter()
                .all(|a| scan::RAW_EXTS.contains(&a.meta.ext.as_str())) =>
        {
            for a in selected {
                resolved.push((PathBuf::from(&a.meta.path), (*a).clone()));
            }
        }
        ExportTarget::Raw => {
            let folder = req
                .raw_folder
                .as_ref()
                .ok_or_else(|| anyhow!("no RAW folder chosen"))?;
            let folder = PathBuf::from(folder);
            if !folder.is_dir() {
                return Err(anyhow!("RAW folder does not exist: {}", folder.display()));
            }
            let index: HashMap<String, PathBuf> = scan::index_raws(&folder, req.raw_recursive)?;
            for p in index.values() {
                if let Some(e) = scan::ext_lower(p) {
                    raw_exts.insert(e.to_uppercase());
                }
            }
            for a in selected {
                match index.get(&a.meta.stem.to_ascii_lowercase()) {
                    Some(raw) => resolved.push((raw.clone(), (*a).clone())),
                    None => unmatched.push(a.meta.file_name.clone()),
                }
            }
        }
    }

    Ok((resolved, unmatched, raw_exts.into_iter().collect()))
}

/// Dry run: report what would be exported.
pub fn preview(items: &[Analysis], req: &ExportRequest) -> Result<MatchReport> {
    let (resolved, unmatched, raw_extensions) = resolve(items, req)?;
    let samples = resolved
        .iter()
        .take(5)
        .map(|(p, a)| {
            [
                a.meta.file_name.clone(),
                p.file_name().unwrap_or_default().to_string_lossy().to_string(),
            ]
        })
        .collect();

    Ok(MatchReport {
        selected: req.ids.len(),
        matched: resolved.len(),
        unmatched,
        samples,
        raw_extensions,
    })
}

/// Perform the export.
pub fn run(items: &[Analysis], req: &ExportRequest) -> Result<ExportReport> {
    let (resolved, unmatched, _) = resolve(items, req)?;
    let mut report = ExportReport {
        matched: resolved.len(),
        exported: 0,
        skipped: 0,
        unmatched,
        errors: Vec::new(),
        destination: req.destination.clone(),
    };

    match req.action {
        ExportAction::ListOnly => {
            let dest = req
                .destination
                .as_ref()
                .ok_or_else(|| anyhow!("no output file chosen"))?;
            let mut out = String::new();
            for (p, _) in &resolved {
                out.push_str(&p.to_string_lossy());
                out.push('\n');
            }
            std::fs::write(dest, out).with_context(|| format!("write {dest}"))?;
            report.exported = resolved.len();
        }

        ExportAction::Sidecar => {
            for (p, a) in &resolved {
                match write_sidecar(p, a) {
                    Ok(()) => report.exported += 1,
                    Err(e) => report
                        .errors
                        .push(format!("{}: {e}", p.file_name().unwrap_or_default().to_string_lossy())),
                }
            }
        }

        ExportAction::Copy | ExportAction::Move => {
            let dest_root = req
                .destination
                .as_ref()
                .ok_or_else(|| anyhow!("no destination folder chosen"))?;
            let dest_root = PathBuf::from(dest_root);
            std::fs::create_dir_all(&dest_root)
                .with_context(|| format!("create {}", dest_root.display()))?;

            let reject_dir = dest_root.join("rejected");
            if req.include_rejects_subfolder {
                std::fs::create_dir_all(&reject_dir)?;
            }

            for (src, a) in &resolved {
                let dir = if req.include_rejects_subfolder && a.verdict == Verdict::Reject {
                    &reject_dir
                } else {
                    &dest_root
                };
                let name = match src.file_name() {
                    Some(n) => n,
                    None => {
                        report.errors.push(format!("{}: no file name", src.display()));
                        continue;
                    }
                };
                let dst = dir.join(name);

                // Refuse to clobber. Silently overwriting someone's RAW files
                // would be unrecoverable.
                if dst.exists() {
                    report.skipped += 1;
                    continue;
                }

                let result = if req.action == ExportAction::Copy {
                    std::fs::copy(src, &dst).map(|_| ())
                } else {
                    move_file(src, &dst)
                };

                match result {
                    Ok(()) => report.exported += 1,
                    Err(e) => report
                        .errors
                        .push(format!("{}: {e}", name.to_string_lossy())),
                }
            }
        }
    }

    Ok(report)
}

/// Rename where possible, falling back to copy+delete across volumes.
fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst)?;
            std::fs::remove_file(src)
        }
    }
}

/// Write an XMP sidecar carrying the star rating and a keep/reject label.
///
/// Lightroom, Capture One and Bridge all read `xmp:Rating` and `xmp:Label`, so
/// this is the least disruptive way to hand a cull back to an existing edit.
/// An existing sidecar is left alone unless it was written by this app.
fn write_sidecar(raw: &Path, a: &Analysis) -> Result<()> {
    let side = raw.with_extension("xmp");

    if side.exists() {
        let existing = std::fs::read_to_string(&side).unwrap_or_default();
        if !existing.contains("culling.app") {
            return Err(anyhow!("sidecar already exists (not written by this app)"));
        }
    }

    let keep = a.effective_keep();
    // Rating precedence: an explicit user rating, else derive from the verdict
    // so a cull is still useful when the user never touched the stars.
    let rating = if a.rating > 0 {
        a.rating
    } else if keep {
        3
    } else {
        0
    };
    let label = if keep { "Keep" } else { "Reject" };

    let reasons = a
        .reasons
        .iter()
        .map(|r| r.label.as_str())
        .collect::<Vec<_>>()
        .join("; ");

    let xml = format!(
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="culling.app">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/">
   <xmp:Rating>{rating}</xmp:Rating>
   <xmp:Label>{label}</xmp:Label>
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">score {score:.0}/100{sep}{reasons}</rdf:li>
    </rdf:Alt>
   </dc:description>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#,
        rating = rating,
        label = label,
        score = a.score,
        sep = if reasons.is_empty() { "" } else { " — " },
        reasons = xml_escape(&reasons),
    );

    std::fs::write(&side, xml).with_context(|| format!("write {}", side.display()))?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Export the full analysis as CSV, for anyone who wants to work in a spreadsheet.
pub fn write_csv(items: &[Analysis], dest: &Path) -> Result<()> {
    let mut out = String::from(
        "file,verdict,score,rating,group,is_group_best,sharpness,eyes,faces,iso,shutter,aperture,reasons\n",
    );
    for a in items {
        let eyes = if a.faces.is_empty() {
            "n/a".to_string()
        } else {
            format!("{:?}", a.faces[0].eye_state).to_lowercase()
        };
        let reasons = a
            .reasons
            .iter()
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!(
            "{},{:?},{:.1},{},{},{},{:.3},{},{},{},{},{},{}\n",
            csv_escape(&a.meta.file_name),
            a.verdict,
            a.score,
            a.rating,
            a.group_id.map(|g| g.to_string()).unwrap_or_default(),
            a.is_group_best,
            a.metrics.sharpness_subject,
            eyes,
            a.faces.len(),
            a.meta.iso.map(|v| v.to_string()).unwrap_or_default(),
            csv_escape(a.meta.shutter.as_deref().unwrap_or("")),
            a.meta.aperture.map(|v| format!("{v:.1}")).unwrap_or_default(),
            csv_escape(&reasons),
        ));
    }
    std::fs::write(dest, out).with_context(|| format!("write {}", dest.display()))?;
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
