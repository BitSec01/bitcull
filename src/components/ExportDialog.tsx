import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { exportPreview, exportRun } from "../api";
import type {
  ExportAction,
  ExportReport,
  ExportRequest,
  ExportTarget,
  MatchReport,
} from "../types";

interface Props {
  /** Ids to export. The backend already holds the full analysis. */
  selectedIds: string[];
  onClose: () => void;
}

/**
 * Export flow.
 *
 * The whole point of the app is that the JPEGs are proxies — what the user
 * needs at the end is the matching RAW files. So RAW is the default target,
 * and nothing is written until the match has been previewed.
 */
export default function ExportDialog({ selectedIds, onClose }: Props) {
  const [target, setTarget] = useState<ExportTarget>("raw");
  const [action, setAction] = useState<ExportAction>("copy");
  const [rawFolder, setRawFolder] = useState("");
  const [rawRecursive, setRawRecursive] = useState(true);
  const [destination, setDestination] = useState("");
  const [includeRejects, setIncludeRejects] = useState(false);
  const [match, setMatch] = useState<MatchReport | null>(null);
  const [report, setReport] = useState<ExportReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const request = (): ExportRequest => ({
    action,
    target,
    rawFolder: rawFolder || null,
    rawRecursive,
    destination: destination || null,
    ids: selectedIds,
    includeRejectsSubfolder: includeRejects,
  });

  // Re-check the match whenever the inputs that affect it change.
  useEffect(() => {
    setReport(null);
    if (target === "raw" && !rawFolder) {
      setMatch(null);
      return;
    }
    let cancelled = false;
    exportPreview(request())
      .then((m) => !cancelled && (setMatch(m), setError(null)))
      .catch((e) => !cancelled && (setMatch(null), setError(String(e))));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target, rawFolder, rawRecursive, selectedIds.length]);

  const needsDestFolder = action === "copy" || action === "move";
  const needsDestFile = action === "listOnly";
  const ready =
    selectedIds.length > 0 &&
    (target === "proxy" || !!rawFolder) &&
    (!needsDestFolder || !!destination) &&
    (!needsDestFile || !!destination) &&
    (match?.matched ?? 0) > 0;

  // Both pickers fall back to the adjacent text field, which accepts a pasted
  // path, so a dialog failure is never a dead end.
  const pickRawFolder = async () => {
    try {
      const dir = await open({ directory: true, title: "Where are the RAW files?" });
      if (typeof dir === "string") setRawFolder(dir);
    } catch (e) {
      setError(`Could not open the folder picker (${e}). Paste the path instead.`);
    }
  };

  const pickDestination = async () => {
    try {
      if (needsDestFile) {
        const f = await save({
          title: "Save file list",
          defaultPath: "keepers.txt",
          filters: [{ name: "Text", extensions: ["txt"] }],
        });
        if (typeof f === "string") setDestination(f);
      } else {
        const dir = await open({ directory: true, title: "Destination folder" });
        if (typeof dir === "string") setDestination(dir);
      }
    } catch (e) {
      setError(`Could not open the file picker (${e}). Paste the path instead.`);
    }
  };

  const doExport = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await exportRun(request()));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <h2>Export {selectedIds.length} photo{selectedIds.length === 1 ? "" : "s"}</h2>
        <p className="lede">
          Your JPEGs are proxies. Point this at your RAW folder and it will
          resolve each pick back to the matching RAW by file name.
        </p>

        {report ? (
          <Result report={report} onClose={onClose} onAgain={() => setReport(null)} />
        ) : (
          <>
            <div className="section">
              <h3>Act on</h3>
              <div className="radio-row">
                <Radio
                  on={target === "raw"}
                  onClick={() => setTarget("raw")}
                  title="The matching RAW files"
                  desc="Resolves 026A2371.JPG to 026A2371.CR3 and acts on the RAW."
                />
                <Radio
                  on={target === "proxy"}
                  onClick={() => setTarget("proxy")}
                  title="The JPEGs themselves"
                  desc="Acts on the proxy files you have been reviewing."
                />
              </div>
            </div>

            {target === "raw" && (
              <div className="field">
                <label>RAW folder</label>
                <div className="path-row">
                  <input
                    type="text"
                    value={rawFolder}
                    placeholder="Choose the folder holding your CR3 / NEF / ARW files"
                    onChange={(e) => setRawFolder(e.target.value)}
                  />
                  <button className="btn" onClick={pickRawFolder}>
                    Browse
                  </button>
                </div>
                <label className="check" style={{ marginTop: 8 }}>
                  <input
                    type="checkbox"
                    checked={rawRecursive}
                    onChange={(e) => setRawRecursive(e.target.checked)}
                  />
                  <span>Search subfolders</span>
                </label>
              </div>
            )}

            <div className="section">
              <h3>What to do</h3>
              <div className="radio-row">
                <Radio
                  on={action === "copy"}
                  onClick={() => setAction("copy")}
                  title="Copy to a folder"
                  desc="Leaves the originals where they are. Never overwrites."
                />
                <Radio
                  on={action === "move"}
                  onClick={() => setAction("move")}
                  title="Move to a folder"
                  desc="Relocates the files. Refuses to overwrite anything."
                />
                <Radio
                  on={action === "sidecar"}
                  onClick={() => setAction("sidecar")}
                  title="Write XMP sidecars"
                  desc="Ratings and Keep/Reject labels next to the originals, for Lightroom or Capture One."
                />
                <Radio
                  on={action === "listOnly"}
                  onClick={() => setAction("listOnly")}
                  title="Just save a list"
                  desc="Writes the full paths to a text file. Changes nothing on disk."
                />
              </div>
            </div>

            {(needsDestFolder || needsDestFile) && (
              <div className="field">
                <label>{needsDestFile ? "Output file" : "Destination folder"}</label>
                <div className="path-row">
                  <input
                    type="text"
                    value={destination}
                    onChange={(e) => setDestination(e.target.value)}
                    placeholder={needsDestFile ? "keepers.txt" : "Where should they go?"}
                  />
                  <button className="btn" onClick={pickDestination}>
                    Browse
                  </button>
                </div>
                {needsDestFolder && (
                  <label className="check" style={{ marginTop: 8 }}>
                    <input
                      type="checkbox"
                      checked={includeRejects}
                      onChange={(e) => setIncludeRejects(e.target.checked)}
                    />
                    <span>Put rejects in a "rejected" subfolder</span>
                  </label>
                )}
              </div>
            )}

            {error && <div className="notice bad">{error}</div>}

            {match && (
              <div className={`notice ${match.unmatched.length ? "" : "good"}`}>
                <strong>
                  {match.matched} of {match.selected} matched
                </strong>
                {match.rawExtensions.length > 0 && (
                  <> · found {match.rawExtensions.join(", ")} in that folder</>
                )}
                {match.samples.length > 0 && (
                  <div style={{ marginTop: 6, opacity: 0.85 }}>
                    e.g. {match.samples[0][0]} → {match.samples[0][1]}
                  </div>
                )}
                {match.unmatched.length > 0 && (
                  <div style={{ marginTop: 6 }}>
                    No RAW found for {match.unmatched.length}:{" "}
                    {match.unmatched.slice(0, 4).join(", ")}
                    {match.unmatched.length > 4 && " …"}
                  </div>
                )}
              </div>
            )}

            <div className="modal-actions">
              <button className="btn ghost" onClick={onClose}>
                Cancel
              </button>
              <button
                className="btn primary"
                onClick={doExport}
                disabled={!ready || busy}
              >
                {busy && <span className="spin" />}
                {action === "move" ? "Move" : action === "copy" ? "Copy" : "Write"}{" "}
                {match?.matched ?? 0} file{(match?.matched ?? 0) === 1 ? "" : "s"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function Radio({
  on,
  onClick,
  title,
  desc,
}: {
  on: boolean;
  onClick: () => void;
  title: string;
  desc: string;
}) {
  return (
    <label className={`radio ${on ? "on" : ""}`} onClick={onClick}>
      <input type="radio" checked={on} readOnly />
      <span>
        <b>{title}</b>
        <small>{desc}</small>
      </span>
    </label>
  );
}

function Result({
  report,
  onClose,
  onAgain,
}: {
  report: ExportReport;
  onClose: () => void;
  onAgain: () => void;
}) {
  const clean = report.errors.length === 0 && report.skipped === 0;
  return (
    <>
      <div className={`notice ${clean ? "good" : ""}`}>
        <strong>{report.exported} exported</strong>
        {report.skipped > 0 && <> · {report.skipped} skipped (already existed)</>}
        {report.unmatched.length > 0 && (
          <> · {report.unmatched.length} had no matching RAW</>
        )}
        {report.destination && (
          <div style={{ marginTop: 6, opacity: 0.85 }}>{report.destination}</div>
        )}
      </div>

      {report.errors.length > 0 && (
        <div className="notice bad">
          <strong>{report.errors.length} failed</strong>
          <div style={{ marginTop: 6 }}>
            {report.errors.slice(0, 6).map((e, i) => (
              <div key={i}>{e}</div>
            ))}
          </div>
        </div>
      )}

      <div className="modal-actions">
        <button className="btn ghost" onClick={onAgain}>
          Export something else
        </button>
        <button className="btn primary" onClick={onClose}>
          Done
        </button>
      </div>
    </>
  );
}
