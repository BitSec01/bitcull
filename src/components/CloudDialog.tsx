import { useState } from "react";
import { cloudVerify, setCloudConfig } from "../api";
import type { CloudConfig, CloudReport } from "../types";

interface Props {
  config: CloudConfig;
  onSaved: (c: CloudConfig) => void;
  onDone: () => void;
  onClose: () => void;
}

/**
 * Optional second opinion on borderline eye-state calls.
 *
 * The framing here matters: this never uploads a photograph. It sends face
 * crops of a couple of hundred pixels, only for frames the local heuristic
 * could not confidently judge, which is why a free tier is enough.
 */
export default function CloudDialog({ config, onSaved, onDone, onClose }: Props) {
  const [cfg, setCfg] = useState<CloudConfig>(config);
  const [report, setReport] = useState<CloudReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async (next: CloudConfig) => {
    setCfg(next);
    await setCloudConfig(next);
    onSaved(next);
  };

  const run = async () => {
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      await setCloudConfig(cfg);
      const r = await cloudVerify();
      setReport(r);
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal">
        <h2>Cloud check for closed eyes</h2>
        <p className="lede">
          Sharpness, exposure and duplicate detection all run locally and are
          better for it. The one judgement a vision model genuinely beats a
          local heuristic at is looking at a small, soft face and saying whether
          the eyes are shut — so that is all this does.
        </p>

        <div className="notice">
          Only <strong>face crops of about 192 px</strong> are uploaded, and only
          for frames the local check was unsure about. A typical shoot sends a
          few dozen requests of a few kilobytes — comfortably inside a free tier.
          Your photographs are never uploaded.
        </div>

        <label className="check">
          <input
            type="checkbox"
            checked={cfg.enabled}
            onChange={(e) => save({ ...cfg, enabled: e.target.checked })}
          />
          <span>
            <b>Enable cloud verification</b>
            Off by default. Nothing leaves your machine until you turn this on.
          </span>
        </label>

        <div className="field">
          <label>Provider</label>
          <select
            value={cfg.provider}
            onChange={(e) =>
              setCfg({ ...cfg, provider: e.target.value as CloudConfig["provider"], model: "" })
            }
            disabled={!cfg.enabled}
          >
            <option value="gemini">Google Gemini — has a free tier</option>
            <option value="anthropic">Anthropic Claude — paid</option>
          </select>
          <div className="hint">
            {cfg.provider === "gemini"
              ? "Free key from aistudio.google.com/apikey. Defaults to gemini-2.0-flash."
              : "Key from console.anthropic.com. Defaults to claude-opus-5."}
          </div>
        </div>

        <div className="field">
          <label>API key</label>
          <input
            type="password"
            value={cfg.apiKey}
            placeholder="Paste your key"
            onChange={(e) => setCfg({ ...cfg, apiKey: e.target.value })}
            disabled={!cfg.enabled}
          />
          <div className="hint">
            Held in memory for this session only — it is never written to disk
            and never sent back to the interface.
          </div>
        </div>

        <div className="field">
          <label>
            Request ceiling <b>{cfg.maxRequests}</b>
          </label>
          <input
            type="range"
            min={10}
            max={500}
            step={10}
            value={cfg.maxRequests}
            onChange={(e) => setCfg({ ...cfg, maxRequests: +e.target.value })}
            disabled={!cfg.enabled}
          />
          <div className="hint">
            A hard stop, so a mistake here cannot run away with your quota.
          </div>
        </div>

        {error && <div className="notice bad">{error}</div>}

        {report && (
          <div className="notice good">
            <strong>
              Checked {report.facesChecked} faces in {report.requests} requests
            </strong>
            {" · "}
            {report.changed} verdict{report.changed === 1 ? "" : "s"} changed
            {report.skippedOverLimit > 0 && (
              <> · {report.skippedOverLimit} skipped at the ceiling</>
            )}
            {report.errors.length > 0 && (
              <div style={{ marginTop: 6 }}>
                {report.errors.length} error(s): {report.errors[0]}
              </div>
            )}
          </div>
        )}

        <div className="modal-actions">
          <button className="btn ghost" onClick={onClose}>
            Close
          </button>
          <button
            className="btn primary"
            onClick={run}
            disabled={!cfg.enabled || !cfg.apiKey || busy}
          >
            {busy && <span className="spin" />}
            Check borderline faces
          </button>
        </div>
      </div>
    </div>
  );
}
