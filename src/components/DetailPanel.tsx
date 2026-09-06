import { useEffect, useState } from "react";
import { previewUrl, revealInFolder } from "../api";
import { orientedAspect } from "../fit";
import type { Analysis, Face, Pick, Subject } from "../types";

/** Mirror of `subjects::prominence` — log scale, 0.15% .. 25% of frame. */
function promScale(area: number): number {
  const pct = Math.max(0.01, area * 100);
  return Math.min(1, Math.max(0, (Math.log10(pct) + 0.82) / 2.22));
}

interface Props {
  item: Analysis | null;
  showBoxes: boolean;
  onPick: (id: string, pick: Pick | null) => void;
  onRate: (id: string, rating: number) => void;
  onOpenLoupe: () => void;
}

export default function DetailPanel({
  item,
  showBoxes,
  onPick,
  onRate,
  onOpenLoupe,
}: Props) {
  // Reset the loaded flag whenever the selection changes so the previous
  // photo's preview is not shown under the new photo's overlays.
  const [loaded, setLoaded] = useState(false);
  useEffect(() => setLoaded(false), [item?.meta.id]);

  if (!item) {
    return (
      <aside className="sidebar right">
        <div className="empty">
          <p>Select a photo to see why it scored the way it did.</p>
        </div>
      </aside>
    );
  }

  const m = item.metrics;
  const meta = item.meta;

  return (
    <aside className="sidebar right">
      <div className="detail">
        <div className="detail-img" onClick={onOpenLoupe} title="Open loupe (Space)">
          <img
            src={previewUrl(item.meta.id, 900)}
            alt={meta.fileName}
            draggable={false}
            onLoad={() => setLoaded(true)}
          />
          {loaded && showBoxes && (
            <div
              className="detail-frame"
              style={{ aspectRatio: String(orientedAspect(meta)) }}
            >
              {item.subjects.slice(0, 20).map((s, i) => (
                <SubjectBox key={`s${i}`} subject={s} />
              ))}
              {item.faces
                .filter((f) => f.area > 0.0008)
                .slice(0, 12)
                .map((f, i) => (
                  <FaceBox key={i} face={f} />
                ))}
            </div>
          )}
        </div>

        <div className="detail-scroll">
          <h2>{meta.fileName}</h2>
          <div className="sub">
            {meta.width}×{meta.height}
            {meta.camera ? ` · ${meta.camera}` : ""}
            {item.groupId !== null &&
              ` · group ${item.groupId}${item.isGroupBest ? " (best)" : ` (#${item.groupRank + 1})`}`}
          </div>

          <div className="verdict-row">
            <button
              className={`verdict-btn keep ${item.userPick === "keep" ? "on" : ""}`}
              onClick={() =>
                onPick(meta.id, item.userPick === "keep" ? null : "keep")
              }
            >
              Keep
            </button>
            <button
              className={`verdict-btn reject ${item.userPick === "reject" ? "on" : ""}`}
              onClick={() =>
                onPick(meta.id, item.userPick === "reject" ? null : "reject")
              }
            >
              Reject
            </button>
          </div>

          <div className="stars">
            {[1, 2, 3, 4, 5].map((n) => (
              <button
                key={n}
                className={`star ${item.rating >= n ? "on" : ""}`}
                onClick={() => onRate(meta.id, item.rating === n ? 0 : n)}
                title={`${n} star${n > 1 ? "s" : ""}`}
              >
                ★
              </button>
            ))}
          </div>

          <div className="section">
            <h3>
              Score {Math.round(item.score)} — {item.verdict}
              {item.userPick && " (manual)"}
            </h3>
            <div className="reasons">
              {item.reasons.length === 0 && (
                <div className="reason">Nothing notable — a clean frame.</div>
              )}
              {item.reasons.map((r, i) => (
                <div key={i} className={`reason ${r.severity}`}>
                  <span>{r.label}</span>
                  {Math.abs(r.delta) >= 0.5 && (
                    <span className="delta">
                      {r.delta > 0 ? "+" : ""}
                      {r.delta.toFixed(0)}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>

          {item.subjects.length > 0 && (
            <div className="section">
              <h3>
                Subject
                {item.subjectStats.count > 0
                  ? ` — ${(item.subjectStats.mainArea * 100).toFixed(1)}% of frame`
                  : " — none counted"}
              </h3>
              <Meter
                label="Subject size"
                value={promScale(item.subjectStats.mainArea)}
                good={(v) => v > 0.5}
              />
              <Meter
                label="Focus on subject"
                value={
                  1 -
                  Math.min(
                    1,
                    Math.max(
                      0,
                      m.sharpnessPeak - item.subjectStats.mainSharpness,
                    ) / 0.35,
                  )
                }
                good={(v) => v > 0.6}
              />
              <Meter
                label="Background separation"
                value={Math.min(
                  1,
                  Math.max(
                    0,
                    item.subjectStats.mainSharpness -
                      item.subjectStats.backgroundSharpness,
                  ) / 0.25,
                )}
                good={(v) => v > 0.4}
              />
              <dl className="kv" style={{ marginTop: 8 }}>
                {Object.entries(
                  item.subjects.reduce<Record<string, number>>((acc, s) => {
                    acc[s.label] = (acc[s.label] ?? 0) + 1;
                    return acc;
                  }, {}),
                )
                  .sort((a, b) => b[1] - a[1])
                  .slice(0, 6)
                  .map(([label, n]) => (
                    <div key={label} style={{ display: "contents" }}>
                      <dt>{label}</dt>
                      <dd>{n}</dd>
                    </div>
                  ))}
              </dl>
            </div>
          )}

          <div className="section">
            <h3>Measurements</h3>
            <Meter
              label="Subject sharpness"
              value={m.sharpnessSubject}
              good={(v) => v > 0.45}
            />
            <Meter
              label="Frame peak sharpness"
              value={m.sharpnessPeak}
              good={(v) => v > 0.45}
            />
            <Meter
              label="Directional blur"
              value={m.motionBlur}
              good={(v) => v < 0.5}
            />
            <Meter
              label="Contrast"
              value={m.exposure.stddev * 3}
              good={(v) => v > 0.3}
            />
            <Meter
              label="Clipped highlights"
              value={m.exposure.clippedHighlights * 5}
              good={(v) => v < 0.2}
            />
            <Meter label="Noise" value={m.noise} good={(v) => v < 0.6} />
          </div>

          {item.faces.length > 0 && (
            <div className="section">
              <h3>Faces ({item.faces.length})</h3>
              <dl className="kv">
                {item.faces
                  .filter((f) => f.area > 0.0008)
                  .slice(0, 6)
                  .map((f, i) => (
                    <FaceRow key={i} face={f} index={i} />
                  ))}
              </dl>
            </div>
          )}

          <div className="section">
            <h3>Capture</h3>
            <dl className="kv">
              {meta.lens && (
                <>
                  <dt>Lens</dt>
                  <dd>{meta.lens}</dd>
                </>
              )}
              {meta.focalLen && (
                <>
                  <dt>Focal length</dt>
                  <dd>{Math.round(meta.focalLen)} mm</dd>
                </>
              )}
              {meta.shutter && (
                <>
                  <dt>Shutter</dt>
                  <dd>{meta.shutter}</dd>
                </>
              )}
              {meta.aperture && (
                <>
                  <dt>Aperture</dt>
                  <dd>f/{meta.aperture.toFixed(1)}</dd>
                </>
              )}
              {meta.iso && (
                <>
                  <dt>ISO</dt>
                  <dd>{meta.iso}</dd>
                </>
              )}
              {meta.takenAt && (
                <>
                  <dt>Taken</dt>
                  <dd>{new Date(meta.takenAt * 1000).toLocaleString()}</dd>
                </>
              )}
              <dt>Size</dt>
              <dd>{(meta.bytes / 1024 / 1024).toFixed(1)} MB</dd>
              <dt>RAW stem</dt>
              <dd>{meta.stem}</dd>
            </dl>
            <button
              className="btn sm ghost"
              style={{ marginTop: 10 }}
              onClick={() => revealInFolder(meta.path).catch(() => {})}
            >
              Show in folder
            </button>
          </div>
        </div>
      </div>
    </aside>
  );
}

function SubjectBox({ subject }: { subject: Subject }) {
  // Only the classes being culled for get a label; the rest are drawn faintly
  // so you can see what the detector found without the frame turning into a
  // wall of boxes.
  return (
    <div
      className={`subject-box ${subject.counts ? "counts" : ""}`}
      style={{
        left: `${subject.x * 100}%`,
        top: `${subject.y * 100}%`,
        width: `${subject.w * 100}%`,
        height: `${subject.h * 100}%`,
      }}
    >
      {subject.counts && subject.area > 0.01 && (
        <i>
          {subject.label} {(subject.area * 100).toFixed(1)}%
        </i>
      )}
    </div>
  );
}

function FaceBox({ face }: { face: Face }) {
  const cls =
    face.eyeState === "closed"
      ? "closed"
      : face.eyeState === "squint"
        ? "squint"
        : "";
  return (
    <div
      className={`face-box ${cls}`}
      style={{
        left: `${face.x * 100}%`,
        top: `${face.y * 100}%`,
        width: `${face.w * 100}%`,
        height: `${face.h * 100}%`,
      }}
    >
      {(face.eyeState === "closed" || face.eyeState === "squint") && (
        <i>{face.eyeState === "closed" ? "eyes shut" : "squint"}</i>
      )}
    </div>
  );
}

function FaceRow({ face, index }: { face: Face; index: number }) {
  const label =
    face.eyeState === "unknown"
      ? "can't tell"
      : face.eyeState === "open"
        ? "eyes open"
        : face.eyeState === "closed"
          ? "eyes closed"
          : "squinting";
  return (
    <>
      <dt>Face {index + 1}</dt>
      <dd>
        {label} · {(face.eyeOpen * 100).toFixed(0)}% open · sharp{" "}
        {face.sharpness.toFixed(2)}
        <br />
        <span style={{ opacity: 0.6 }}>
          {face.eyeSource === "cloud"
            ? "verified by cloud AI"
            : face.eyeSource === "model"
              ? "local model"
              : "local heuristic"}
        </span>
      </dd>
    </>
  );
}

function Meter({
  label,
  value,
  good,
}: {
  label: string;
  value: number;
  good: (v: number) => boolean;
}) {
  const v = Math.max(0, Math.min(1, value));
  const colour = good(v) ? "var(--keep)" : v > 0.75 ? "var(--reject)" : "var(--maybe)";
  return (
    <div className="meter">
      <div className="meter-head">
        <span>{label}</span>
        <b>{v.toFixed(2)}</b>
      </div>
      <div className="meter-track">
        <div
          className="meter-fill"
          style={{ width: `${v * 100}%`, background: colour }}
        />
      </div>
    </div>
  );
}
