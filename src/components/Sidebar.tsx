import { useState } from "react";
import type { ScoreSettings } from "../types";

/**
 * COCO class groups, so the same scoring works for whatever you shoot next.
 * The indices are the model's own class ids — see `subjects.rs` for the list.
 */
const SUBJECT_PRESETS: {
  key: string;
  label: string;
  hint: string;
  classes: number[];
}[] = [
  {
    key: "people",
    label: "People",
    hint: "person, plus the ball they are chasing",
    classes: [0, 32],
  },
  {
    key: "animals",
    label: "Birds & animals",
    hint: "bird, cat, dog, horse, sheep, cow, elephant, bear, zebra, giraffe",
    classes: [14, 15, 16, 17, 18, 19, 20, 21, 22, 23],
  },
  {
    key: "vehicles",
    label: "Vehicles",
    hint: "bicycle, car, motorcycle, aeroplane, bus, train, truck, boat",
    classes: [1, 2, 3, 4, 5, 6, 7, 8],
  },
  {
    key: "boards",
    label: "Board & racket",
    hint: "skis, snowboard, skateboard, surfboard, tennis racket, frisbee, kite",
    classes: [29, 30, 31, 33, 36, 37, 38],
  },
];

/**
 * How harsh the cull is, as one control.
 *
 * Two separate threshold numbers were the single most confusing thing in the
 * old panel — nobody should have to reason about where 72 sits on an invisible
 * scale. One slider moves both, and the labels say what you get.
 */
const STRICTNESS = [
  { label: "Very lenient", keep: 55, reject: 30, blurb: "Keeps most frames. Use for a first pass." },
  { label: "Lenient", keep: 63, reject: 40, blurb: "Cuts only the clearly weak frames." },
  { label: "Balanced", keep: 72, reject: 50, blurb: "A normal cull." },
  { label: "Strict", keep: 79, reject: 58, blurb: "Only frames with a decent, sharp subject." },
  { label: "Very strict", keep: 85, reject: 65, blurb: "The handful of genuinely good ones." },
];

function strictnessIndex(s: ScoreSettings): number {
  // Nearest preset by keep threshold, so a hand-tuned value still shows a
  // sensible position rather than snapping the slider to an end.
  let best = 2;
  let dist = Infinity;
  STRICTNESS.forEach((p, i) => {
    const d = Math.abs(p.keep - s.keepThreshold);
    if (d < dist) {
      dist = d;
      best = i;
    }
  });
  return best;
}

interface Props {
  settings: ScoreSettings;
  onChange: (s: ScoreSettings) => void;
  onReset: () => void;
  cacheBytes: number;
  onPurgeCache: () => void;
  busy: boolean;
  /** Live verdict counts, so a change shows its effect immediately. */
  counts: { keep: number; maybe: number; reject: number; total: number };
  /** Hidden on narrow windows. Kept mounted so its scroll position survives. */
  collapsed: boolean;
}

export default function Sidebar({
  settings,
  onChange,
  onReset,
  cacheBytes,
  onPurgeCache,
  busy,
  counts,
  collapsed,
}: Props) {
  const [advanced, setAdvanced] = useState(false);
  const set = <K extends keyof ScoreSettings>(k: K, v: ScoreSettings[K]) =>
    onChange({ ...settings, [k]: v });

  const si = strictnessIndex(settings);

  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      {/* ---------------------------------------------------- what you shoot */}
      <div className="section">
        <h3>1 · What are you shooting?</h3>
        <p className="explain">
          Everything below is judged against this. The app looks for these
          things in each frame and asks how big and how sharp they are.
        </p>
        <div className="preset-grid">
          {SUBJECT_PRESETS.map((p) => {
            const on = p.classes.every((c) => settings.subjectClasses.includes(c));
            return (
              <button
                key={p.key}
                className={`preset ${on ? "on" : ""}`}
                disabled={busy}
                title={p.hint}
                onClick={() => {
                  const cur = new Set(settings.subjectClasses);
                  if (on) p.classes.forEach((c) => cur.delete(c));
                  else p.classes.forEach((c) => cur.add(c));
                  set("subjectClasses", [...cur].sort((a, b) => a - b));
                }}
              >
                {p.label}
              </button>
            );
          })}
        </div>
        {settings.subjectClasses.length === 0 && (
          <div className="warn-note">
            Nothing selected, so no frame has a subject and scores fall back to
            technical quality only.
          </div>
        )}
      </div>

      {/* ------------------------------------------------------- strictness */}
      <div className="section">
        <h3>2 · How harsh should the cull be?</h3>
        <input
          type="range"
          min={0}
          max={STRICTNESS.length - 1}
          step={1}
          value={si}
          disabled={busy}
          onChange={(e) => {
            const p = STRICTNESS[+e.target.value];
            onChange({
              ...settings,
              keepThreshold: p.keep,
              rejectThreshold: p.reject,
            });
          }}
        />
        <div className="range-ends">
          <span>Keep more</span>
          <span>Keep fewer</span>
        </div>
        <div className="strictness-read">
          <b>{STRICTNESS[si].label}</b>
          <span>{STRICTNESS[si].blurb}</span>
        </div>
        {counts.total > 0 && (
          <div className="outcome">
            <span className="pill keep">{counts.keep} keep</span>
            <span className="pill maybe">{counts.maybe} maybe</span>
            <span className="pill reject">{counts.reject} cut</span>
          </div>
        )}
      </div>

      {/* ---------------------------------------------------- what counts */}
      <div className="section">
        <h3>3 · What makes a good photo?</h3>

        <Weight
          label="Subject size &amp; focus"
          value={settings.wSubject}
          max={1}
          onChange={(v) => set("wSubject", v)}
          busy={busy}
          help="How much of the frame the subject fills, and whether focus landed on it. This is the big one for sports — a close, sharp player beats a wide shot of distant specks."
        />
        <Weight
          label="Overall sharpness"
          value={settings.wSharpness}
          max={2}
          onChange={(v) => set("wSharpness", v)}
          busy={busy}
          help="Spreads the scores apart by how sharp the subject is compared with the rest of the shoot. Turn down if you want size alone to decide."
        />
        <Weight
          label="Exposure"
          value={settings.wExposure}
          max={2}
          onChange={(v) => set("wExposure", v)}
          busy={busy}
          help="Penalises blown highlights, crushed shadows and very flat frames."
        />
        <Weight
          label="Eyes open"
          value={settings.wFaces}
          max={2}
          onChange={(v) => set("wFaces", v)}
          busy={busy}
          help="Only does anything if you ticked 'Check eyes' when opening the folder. Weak signal in field sport, where players rarely face the camera."
        />
        <Weight
          label="Noise &amp; clutter"
          value={settings.wComposition}
          max={2}
          onChange={(v) => set("wComposition", v)}
          busy={busy}
          help="Penalises heavy grain and frames where nothing at all is in focus."
        />
      </div>

      {/* -------------------------------------------------------- grouping */}
      <div className="section">
        <h3>4 · Stacking bursts</h3>
        <p className="explain">
          Frames from one press of the shutter are stacked together and the best
          one is picked, so you judge each moment once.
        </p>

        <label className="check">
          <input
            type="checkbox"
            checked={settings.groupByTime}
            onChange={(e) => set("groupByTime", e.target.checked)}
            disabled={busy}
          />
          <span>
            <b>Shot at almost the same moment</b>
            Within {settings.burstGapSecs.toFixed(2)}s of each other.
          </span>
        </label>

        <div className="field" style={{ opacity: settings.groupByTime ? 1 : 0.4 }}>
          <input
            type="range"
            min={0.05}
            max={5}
            step={0.05}
            value={settings.burstGapSecs}
            onChange={(e) => set("burstGapSecs", +e.target.value)}
            disabled={busy || !settings.groupByTime}
          />
          <div className="range-ends">
            <span>0.05s</span>
            <span>5s</span>
          </div>
          <div className="hint">
            Your camera fires about every 0.1s on continuous drive, so anything
            under ~0.2s is one burst. Above a second, a whole passage of play
            collapses into a single stack.
          </div>
        </div>

        <label className="check">
          <input
            type="checkbox"
            checked={settings.groupByAppearance}
            onChange={(e) => set("groupByAppearance", e.target.checked)}
            disabled={busy}
          />
          <span>
            <b>And they look alike</b>
            Guards against stacking two different moments that happen to be
            close in time.
          </span>
        </label>

        <label className="check">
          <input
            type="checkbox"
            checked={settings.keepOnePerGroup}
            onChange={(e) => set("keepOnePerGroup", e.target.checked)}
            disabled={busy}
          />
          <span>
            <b>Cut the runners-up</b>
            Mark the losers of each stack as duplicates. Untick to judge every
            frame on its own.
          </span>
        </label>
      </div>

      {/* -------------------------------------------------------- advanced */}
      <div className="section">
        <button className="disclosure" onClick={() => setAdvanced((v) => !v)}>
          {advanced ? "▾" : "▸"} Advanced
        </button>

        {advanced && (
          <div className="advanced">
            <div className="field">
              <label>
                Softness cutoff <b>{settings.sharpnessFloor.toFixed(2)}</b>
              </label>
              <input
                type="range"
                min={0.1}
                max={0.7}
                step={0.01}
                value={settings.sharpnessFloor}
                onChange={(e) => set("sharpnessFloor", +e.target.value)}
                disabled={busy}
              />
              <div className="hint">
                Below this a subject is called out as soft. Fixed scale, so a
                cleanly focused shoot is never accused of being blurry.
              </div>
            </div>

            <div className="field">
              <label>
                How sure the detector must be{" "}
                <b>{Math.round(settings.subjectConfidence * 100)}%</b>
              </label>
              <input
                type="range"
                min={0.15}
                max={0.7}
                step={0.05}
                value={settings.subjectConfidence}
                onChange={(e) => set("subjectConfidence", +e.target.value)}
                disabled={busy}
              />
              <div className="hint">
                Lower finds more distant figures but invents a few. Needs a
                re-analyse to take effect — everything else here is instant.
              </div>
            </div>

            <div className="field">
              <label>
                How alike counts as a duplicate <b>{settings.hashDistance}</b>
              </label>
              <input
                type="range"
                min={2}
                max={20}
                step={1}
                value={settings.hashDistance}
                onChange={(e) => set("hashDistance", +e.target.value)}
                disabled={busy || !settings.groupByAppearance}
              />
              <div className="hint">Higher stacks more aggressively.</div>
            </div>

            <label className="check">
              <input
                type="checkbox"
                checked={settings.strictEyes}
                onChange={(e) => set("strictEyes", e.target.checked)}
                disabled={busy}
              />
              <span>
                <b>A blink is fatal</b>
                Cut outright when the main subject's eyes are shut.
              </span>
            </label>

            <div className="hint" style={{ marginTop: 12 }}>
              {(cacheBytes / 1024 / 1024).toFixed(1)} MB of thumbnails and
              results cached. Re-opening a folder reuses them.
            </div>
            <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
              <button className="btn sm" onClick={onReset} disabled={busy}>
                Reset settings
              </button>
              <button className="btn sm" onClick={onPurgeCache} disabled={busy}>
                Clear cache
              </button>
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}

/**
 * A weighting control that says what it does.
 *
 * Shown as Off / Low / Normal / High rather than a bare number, because "0.6×"
 * communicates nothing about the effect on the cull.
 */
function Weight({
  label,
  value,
  max,
  onChange,
  busy,
  help,
}: {
  label: string;
  value: number;
  max: number;
  onChange: (v: number) => void;
  busy: boolean;
  help: string;
}) {
  const pct = value / max;
  const word =
    value <= 0.001
      ? "Off"
      : pct < 0.25
        ? "Barely"
        : pct < 0.55
          ? "Some"
          : pct < 0.85
            ? "A lot"
            : "Everything";

  return (
    <div className="field weight">
      <label>
        <span dangerouslySetInnerHTML={{ __html: label }} />
        <b className={value <= 0.001 ? "off" : ""}>{word}</b>
      </label>
      <input
        type="range"
        min={0}
        max={max}
        step={max / 20}
        value={value}
        onChange={(e) => onChange(+e.target.value)}
        disabled={busy}
      />
      <div className="hint">{help}</div>
    </div>
  );
}
