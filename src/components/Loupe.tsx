import { useEffect, useState } from "react";
import { previewUrl } from "../api";
import { orientedAspect } from "../fit";
import type { Analysis, Pick } from "../types";

interface Props {
  items: Analysis[];
  index: number;
  groupMembers: Analysis[];
  onIndex: (i: number) => void;
  onPick: (id: string, pick: Pick | null) => void;
  onClose: () => void;
  showBoxes: boolean;
  onToggleBoxes: () => void;
}

/**
 * Full-screen review.
 *
 * Compare mode is the reason this exists: when the app has grouped eight
 * near-identical frames and picked one, the only way to trust that choice is to
 * see the contenders side by side at size.
 */
export default function Loupe({
  items,
  index,
  groupMembers,
  onIndex,
  onPick,
  onClose,
  showBoxes,
  onToggleBoxes,
}: Props) {
  const [compare, setCompare] = useState(false);
  const item = items[index];

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") return onClose();
      if (e.key === "c" || e.key === "C") return setCompare((v) => !v);
      if (e.key === "b" || e.key === "B") return onToggleBoxes();
      if (e.key === "ArrowRight") return onIndex(Math.min(items.length - 1, index + 1));
      if (e.key === "ArrowLeft") return onIndex(Math.max(0, index - 1));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, items.length, onIndex, onClose]);

  if (!item) return null;

  const canCompare = groupMembers.length > 1;
  const panes = compare && canCompare ? groupMembers.slice(0, 4) : [item];

  return (
    <div className="loupe">
      <button className="btn loupe-close" onClick={onClose}>
        Close ✕
      </button>

      <div
        className="loupe-stage"
        style={{ gridTemplateColumns: `repeat(${panes.length}, minmax(0, 1fr))` }}
      >
        {panes.map((p) => (
          <div key={p.meta.id} className="loupe-pane">
            {/* The frame is sized to the photo's own aspect ratio so the image
                fills it exactly. That lets detection boxes be positioned as
                plain percentages instead of guessing where `contain` put it. */}
            <div
              className="loupe-frame"
              style={{ aspectRatio: String(orientedAspect(p.meta)) }}
            >
              <img
                // A smaller decode per pane in compare mode: four 2200 px
                // previews at once is a lot of work for no visible gain.
                src={previewUrl(p.meta.id, panes.length > 1 ? 1400 : 2200)}
                alt={p.meta.fileName}
                draggable={false}
              />
              {showBoxes &&
                p.subjects.map((s, i) => (
                  <div
                    key={i}
                    className={`subject-box ${s.counts ? "counts" : ""}`}
                    style={{
                      left: `${s.x * 100}%`,
                      top: `${s.y * 100}%`,
                      width: `${s.w * 100}%`,
                      height: `${s.h * 100}%`,
                    }}
                  >
                    {s.counts && s.area > 0.004 && (
                      <i>
                        {s.label} {(s.area * 100).toFixed(1)}%
                      </i>
                    )}
                  </div>
                ))}
            </div>
            {panes.length > 1 && (
              <div className="loupe-tag">
                {p.isGroupBest ? "★ " : ""}
                {p.meta.fileName} · {Math.round(p.score)}
              </div>
            )}
          </div>
        ))}
      </div>

      <div className="loupe-bar">
        <button
          className="btn"
          onClick={() => onIndex(Math.max(0, index - 1))}
          disabled={index === 0}
        >
          ← Prev
        </button>
        <button
          className="btn"
          onClick={() => onIndex(Math.min(items.length - 1, index + 1))}
          disabled={index >= items.length - 1}
        >
          Next →
        </button>

        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600 }}>{item.meta.fileName}</div>
          <div style={{ color: "var(--fg-faint)", fontSize: 11.5 }}>
            {index + 1} of {items.length} · score {Math.round(item.score)} ·{" "}
            {item.reasons.map((r) => r.label).join(" · ") || "clean frame"}
          </div>
        </div>

        <div className="spacer" />

        <button
          className={`btn ${showBoxes ? "" : "ghost"}`}
          onClick={onToggleBoxes}
          title="Show what the detector found"
        >
          {showBoxes ? "▣" : "▢"} Subjects <span className="kbd">B</span>
        </button>

        {canCompare && (
          <button className="btn" onClick={() => setCompare((v) => !v)}>
            {compare ? "Single" : `Compare ${groupMembers.length}`}{" "}
            <span className="kbd">C</span>
          </button>
        )}

        <button
          className={`verdict-btn keep ${item.userPick === "keep" ? "on" : ""}`}
          style={{ flex: "0 0 auto", padding: "6px 16px" }}
          onClick={() => onPick(item.meta.id, item.userPick === "keep" ? null : "keep")}
        >
          Keep <span className="kbd">P</span>
        </button>
        <button
          className={`verdict-btn reject ${item.userPick === "reject" ? "on" : ""}`}
          style={{ flex: "0 0 auto", padding: "6px 16px" }}
          onClick={() => onPick(item.meta.id, item.userPick === "reject" ? null : "reject")}
        >
          Reject <span className="kbd">X</span>
        </button>
      </div>
    </div>
  );
}
