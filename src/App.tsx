import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as api from "./api";
import PhotoGrid, { type GridEntry } from "./components/PhotoGrid";
import DetailPanel from "./components/DetailPanel";
import Sidebar from "./components/Sidebar";
import ExportDialog from "./components/ExportDialog";
import CloudDialog from "./components/CloudDialog";
import Loupe from "./components/Loupe";
import type {
  Analysis,
  CloudConfig,
  Pick,
  Progress,
  RunSummary,
  ScoreSettings,
} from "./types";

type Filter = "all" | "keep" | "maybe" | "reject" | "picks" | "bestOnly";
type Sort = "capture" | "scoreDesc" | "scoreAsc" | "name";

/**
 * Widths at which panels stop earning their space.
 *
 * Both side panels together are ~600 px. Below these the photo grid gets too
 * narrow to review from, so a panel folds away rather than squeezing it — but
 * only when the window actually crosses the threshold, so a deliberate toggle
 * is never undone underneath the user.
 */
const LEFT_MIN = 1120;
const RIGHT_MIN = 900;
/** Below this the toolbar sheds its optional controls. */
const COMPACT_MIN = 1280;

const DEFAULT_SETTINGS: ScoreSettings = {
  sharpnessFloor: 0.32,
  keepThreshold: 72,
  rejectThreshold: 50,
  wSharpness: 1,
  wExposure: 0.6,
  wFaces: 0.35,
  wComposition: 0.3,
  wSubject: 0.8,
  subjectPolicy: "auto",
  subjectClasses: [0, 32],
  subjectConfidence: 0.35,
  burstGapSecs: 0.5,
  groupByTime: true,
  groupByAppearance: true,
  hashDistance: 10,
  keepOnePerGroup: true,
  strictEyes: false,
};

export default function App() {
  const [folder, setFolder] = useState<string | null>(null);
  const [items, setItems] = useState<Analysis[]>([]);
  const [summary, setSummary] = useState<RunSummary | null>(null);
  const [settings, setSettings] = useState<ScoreSettings>(DEFAULT_SETTINGS);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<Sort>("capture");
  const [cardSize, setCardSize] = useState(220);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const [loupeIndex, setLoupeIndex] = useState<number | null>(null);

  const [showExport, setShowExport] = useState(false);
  const [showCloud, setShowCloud] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const [cloudCfg, setCloudCfg] = useState<CloudConfig>({
    enabled: false,
    provider: "gemini",
    apiKey: "",
    model: "",
    maxRequests: 120,
  });
  const [cacheBytes, setCacheBytes] = useState(0);
  const [recursive, setRecursive] = useState(false);
  // Faces are off by default: in field sport the players rarely face the
  // camera and their faces are small, so eye state adds noise rather than
  // signal. Subject detection is what does the work.
  const [detectFaces, setDetectFaces] = useState(false);
  const [detectSubjects, setDetectSubjects] = useState(true);
  const [manualPath, setManualPath] = useState("");
  const [stackBursts, setStackBursts] = useState(true);
  const [showBoxes, setShowBoxes] = useState(true);
  const [expandedGroups, setExpandedGroups] = useState<Set<number>>(new Set());
  const [update, setUpdate] = useState<api.UpdateInfo | null>(null);
  const [leftOpen, setLeftOpen] = useState(window.innerWidth >= LEFT_MIN);
  const [rightOpen, setRightOpen] = useState(window.innerWidth >= RIGHT_MIN);
  const [compact, setCompact] = useState(window.innerWidth < COMPACT_MIN);

  const refreshCache = useCallback(() => {
    api.cacheInfo().then(setCacheBytes).catch(() => {});
  }, []);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    api.getCloudConfig().then(setCloudCfg).catch(() => {});
    refreshCache();
  }, [refreshCache]);

  // One quiet check at startup. Failure is silent by design — being offline is
  // not something the user needs telling about.
  useEffect(() => {
    api
      .checkForUpdate()
      .then((u) => {
        if (u?.newer) setUpdate(u);
      })
      .catch(() => {});
  }, []);

  // Fold panels away as the window narrows, and bring them back when it widens.
  // Acting only on threshold *crossings* means a manual toggle survives every
  // resize that does not change which regime the window is in.
  useEffect(() => {
    let prev = window.innerWidth;
    const onResize = () => {
      const w = window.innerWidth;
      setCompact(w < COMPACT_MIN);
      if (prev >= LEFT_MIN && w < LEFT_MIN) setLeftOpen(false);
      if (prev < LEFT_MIN && w >= LEFT_MIN) setLeftOpen(true);
      if (prev >= RIGHT_MIN && w < RIGHT_MIN) setRightOpen(false);
      if (prev < RIGHT_MIN && w >= RIGHT_MIN) setRightOpen(true);
      prev = w;
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    const un = api.onProgress(setProgress);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  // -------------------------------------------------------------- actions

  const chooseFolder = async () => {
    // Never let this fail silently. A missing Tauri capability rejects the
    // dialog call, and without a catch the button simply does nothing — which
    // is indistinguishable from the app being broken.
    try {
      const dir = await open({
        directory: true,
        title: "Choose a folder of photos",
      });
      if (typeof dir === "string") {
        setFolder(dir);
        runAnalysis(dir, false);
      }
    } catch (e) {
      setError(
        `Could not open the folder picker (${e}). Paste a folder path below instead.`,
      );
    }
  };

  const openTypedPath = () => {
    const dir = manualPath.trim().replace(/^"|"$/g, "");
    if (!dir) return;
    setFolder(dir);
    runAnalysis(dir, false);
  };

  const runAnalysis = async (dir: string, force: boolean) => {
    setBusy(true);
    setError(null);
    setProgress(null);
    try {
      const s = await api.analyzeFolder(dir, recursive, detectFaces, detectSubjects, force);
      setSummary(s);
      setItems(s.items);
      setSelection(new Set());
      setSelectedId(s.items[0]?.meta.id ?? null);
      if (s.modelError) {
        setError(`Some models could not be loaded (${s.modelError}). Everything they feed is skipped; the rest of the analysis ran normally.`);
      }
      refreshCache();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  const applySettings = async (s: ScoreSettings) => {
    setSettings(s);
    try {
      const r = await api.updateSettings(s);
      setSummary(r);
      setItems(r.items);
    } catch (e) {
      setError(String(e));
    }
  };

  const pick = async (id: string, p: Pick | null) => {
    try {
      await api.setPick(id, p);
      // A pick can change which frame wins its group, so pull the whole set.
      const fresh = await api.getItems();
      setItems(fresh);
      setSummary((prev) => (prev ? { ...prev, items: fresh } : prev));
    } catch (e) {
      setError(String(e));
    }
  };

  const rate = async (id: string, rating: number) => {
    await api.setRating(id, rating).catch(() => {});
    setItems((prev) =>
      prev.map((a) => (a.meta.id === id ? { ...a, rating } : a)),
    );
  };

  const bulkPick = async (p: Pick | null) => {
    const ids = selection.size > 0 ? [...selection] : view.map((a) => a.meta.id);
    if (ids.length === 0) return;
    const r = await api.setPicks(ids, p);
    setSummary(r);
    setItems(r.items);
  };

  // ---------------------------------------------------------------- view

  const view = useMemo(() => {
    let out = items;
    switch (filter) {
      case "keep":
        out = out.filter((a) => a.verdict === "keep");
        break;
      case "maybe":
        out = out.filter((a) => a.verdict === "maybe");
        break;
      case "reject":
        out = out.filter((a) => a.verdict === "reject");
        break;
      case "picks":
        out = out.filter((a) => a.userPick !== null);
        break;
      case "bestOnly":
        out = out.filter((a) => a.groupId === null || a.isGroupBest);
        break;
    }
    const sorted = [...out];
    switch (sort) {
      case "scoreDesc":
        sorted.sort((a, b) => b.score - a.score);
        break;
      case "scoreAsc":
        sorted.sort((a, b) => a.score - b.score);
        break;
      case "name":
        sorted.sort((a, b) => a.meta.fileName.localeCompare(b.meta.fileName));
        break;
      default:
        sorted.sort((a, b) => {
          const ta = a.meta.takenAt ?? Number.MAX_SAFE_INTEGER;
          const tb = b.meta.takenAt ?? Number.MAX_SAFE_INTEGER;
          if (ta !== tb) return ta - tb;
          return a.meta.fileName.localeCompare(b.meta.fileName);
        });
    }
    return sorted;
  }, [items, filter, sort]);

  /**
   * Collapse each burst to its best frame, Lightroom-style.
   *
   * Built from `view` so filters and sorting still apply, and expansion inserts
   * a group's other frames directly after their head rather than leaving them
   * scattered wherever the sort would have put them.
   */
  const entries = useMemo<GridEntry[]>(() => {
    if (!stackBursts) {
      return view.map((item) => ({
        item,
        stackCount: 1,
        expanded: false,
        isChild: false,
      }));
    }

    // Group sizes come from the full set: a filter may hide some members, but
    // the stack should still say how many frames the burst really has.
    const sizes = new Map<number, number>();
    for (const a of items) {
      if (a.groupId !== null) sizes.set(a.groupId, (sizes.get(a.groupId) ?? 0) + 1);
    }

    const out: GridEntry[] = [];
    const emitted = new Set<number>();

    for (const item of view) {
      const g = item.groupId;
      if (g === null) {
        out.push({ item, stackCount: 1, expanded: false, isChild: false });
        continue;
      }
      if (emitted.has(g)) continue;
      emitted.add(g);

      const members = view
        .filter((a) => a.groupId === g)
        .sort((a, b) => a.groupRank - b.groupRank);
      // The filter may have hidden the group's best frame; lead with whatever
      // survived rather than dropping the stack entirely.
      const head = members[0] ?? item;
      const isOpen = expandedGroups.has(g);

      out.push({
        item: head,
        stackCount: sizes.get(g) ?? members.length,
        expanded: isOpen,
        isChild: false,
      });

      if (isOpen) {
        for (const m of members.slice(1)) {
          out.push({ item: m, stackCount: 1, expanded: false, isChild: true });
        }
      }
    }
    return out;
  }, [view, items, stackBursts, expandedGroups]);

  const toggleStack = useCallback((groupId: number) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      next.has(groupId) ? next.delete(groupId) : next.add(groupId);
      return next;
    });
  }, []);

  const selected = useMemo(
    () => items.find((a) => a.meta.id === selectedId) ?? null,
    [items, selectedId],
  );

  const groupMembers = useMemo(() => {
    if (!selected || selected.groupId === null) return selected ? [selected] : [];
    return items
      .filter((a) => a.groupId === selected.groupId)
      .sort((a, b) => a.groupRank - b.groupRank);
  }, [items, selected]);

  const counts = useMemo(
    () => ({
      all: items.length,
      keep: items.filter((a) => a.verdict === "keep").length,
      maybe: items.filter((a) => a.verdict === "maybe").length,
      reject: items.filter((a) => a.verdict === "reject").length,
      picks: items.filter((a) => a.userPick !== null).length,
      bestOnly: items.filter((a) => a.groupId === null || a.isGroupBest).length,
    }),
    [items],
  );

  // ------------------------------------------------------------ selection

  const lastIndexRef = useRef(0);

  const handleSelect = (id: string, index: number, additive: boolean) => {
    setSelectedId(id);
    lastIndexRef.current = index;
    setSelection((prev) => {
      if (!additive) return new Set([id]);
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  // Navigate what is actually on screen. With stacks collapsed the arrow keys
  // should skip the hidden frames, not walk through them invisibly.
  const move = useCallback(
    (delta: number) => {
      if (entries.length === 0) return;
      const cur = entries.findIndex((e) => e.item.meta.id === selectedId);
      const next = Math.max(
        0,
        Math.min(entries.length - 1, (cur < 0 ? 0 : cur) + delta),
      );
      const id = entries[next].item.meta.id;
      setSelectedId(id);
      setSelection(new Set([id]));
    },
    [entries, selectedId],
  );

  // ------------------------------------------------------------ keyboard

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
      if (showExport || showCloud) return;

      // The loupe owns navigation while it is open.
      if (loupeIndex !== null) {
        if (e.key === "p" || e.key === "P") {
          const it = view[loupeIndex];
          if (it) pick(it.meta.id, it.userPick === "keep" ? null : "keep");
        }
        if (e.key === "x" || e.key === "X") {
          const it = view[loupeIndex];
          if (it) pick(it.meta.id, it.userPick === "reject" ? null : "reject");
        }
        return;
      }

      switch (e.key) {
        case "ArrowRight":
          e.preventDefault();
          move(1);
          break;
        case "ArrowLeft":
          e.preventDefault();
          move(-1);
          break;
        case "ArrowDown":
          e.preventDefault();
          move(6);
          break;
        case "ArrowUp":
          e.preventDefault();
          move(-6);
          break;
        case " ": {
          e.preventDefault();
          const i = view.findIndex((a) => a.meta.id === selectedId);
          if (i >= 0) setLoupeIndex(i);
          break;
        }
        case "p":
        case "P":
          if (selected) pick(selected.meta.id, selected.userPick === "keep" ? null : "keep");
          break;
        case "x":
        case "X":
          if (selected)
            pick(selected.meta.id, selected.userPick === "reject" ? null : "reject");
          break;
        case "u":
        case "U":
          if (selected) pick(selected.meta.id, null);
          break;
        case "?":
          setShowHelp((v) => !v);
          break;
        default:
          if (selected && /^[0-5]$/.test(e.key)) rate(selected.meta.id, +e.key);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, selectedId, selected, loupeIndex, showExport, showCloud, move]);

  // ------------------------------------------------------------ rendering

  const exportIds =
    selection.size > 1 ? [...selection] : view.filter((a) => a.verdict === "keep").map((a) => a.meta.id);

  const pct = progress ? (progress.done / Math.max(1, progress.total)) * 100 : 0;

  return (
    <div className="app">
      <header className="topbar">
        <button
          className={`btn ghost panel-toggle ${leftOpen ? "on" : ""}`}
          onClick={() => setLeftOpen((v) => !v)}
          title={leftOpen ? "Hide settings" : "Show settings"}
        >
          ☰
        </button>

        {!compact && (
          <div className="brand">
            <span className="brand-mark">◎</span> Culling
          </div>
        )}

        <button className="btn" onClick={chooseFolder} disabled={busy}>
          Open folder…
        </button>

        {folder && (
          <div className="folder-chip" title={folder}>
            <strong>{folder.split(/[\\/]/).pop()}</strong>
            <span>{items.length} photos</span>
          </div>
        )}

        {folder && (
          <button
            className="btn"
            onClick={() => runAnalysis(folder, true)}
            disabled={busy}
            title="Ignore cached results and analyse everything again"
          >
            Re-analyse
          </button>
        )}

        {busy && (
          <button className="btn danger" onClick={() => api.cancelAnalysis()}>
            Stop
          </button>
        )}

        <div className="spacer" />

        {items.length > 0 && (
          <>
            <div className="filters">
              {(
                [
                  ["all", "All"],
                  ["keep", "Keep"],
                  ["maybe", "Maybe"],
                  ["reject", "Reject"],
                  ["bestOnly", "Best of burst"],
                  ["picks", "Marked"],
                ] as [Filter, string][]
              ).map(([k, label]) => (
                <button
                  key={k}
                  className={`chip ${k} ${filter === k ? "on" : ""}`}
                  onClick={() => setFilter(k)}
                >
                  {label}
                  <span className="count">{counts[k]}</span>
                </button>
              ))}
            </div>

            <select
              value={sort}
              onChange={(e) => setSort(e.target.value as Sort)}
              style={{ width: "auto" }}
              title="Sort order"
            >
              <option value="capture">Capture order</option>
              <option value="scoreDesc">Best first</option>
              <option value="scoreAsc">Worst first</option>
              <option value="name">File name</option>
            </select>

            <button
              className={`btn ${stackBursts ? "" : "ghost"}`}
              onClick={() => setStackBursts((v) => !v)}
              title="Collapse each burst to its best frame"
            >
              {stackBursts ? "▣" : "▢"} Stack bursts
            </button>

            <button
              className={`btn ${showBoxes ? "" : "ghost"}`}
              onClick={() => setShowBoxes((v) => !v)}
              title="Outline what the detector found (B)"
            >
              {showBoxes ? "▣" : "▢"} Subjects
            </button>

            {stackBursts && expandedGroups.size > 0 && (
              <button
                className="btn ghost sm"
                onClick={() => setExpandedGroups(new Set())}
                title="Collapse every expanded stack"
              >
                Collapse all
              </button>
            )}

            {!compact && (
              <input
                type="range"
                min={130}
                max={380}
                step={10}
                value={cardSize}
                onChange={(e) => setCardSize(+e.target.value)}
                style={{ width: 84 }}
                title="Thumbnail size"
              />
            )}

            {!compact && (
              <button className="btn" onClick={() => setShowCloud(true)}>
                Cloud check
              </button>
            )}
            <button
              className="btn primary"
              onClick={() => setShowExport(true)}
              disabled={exportIds.length === 0}
            >
              Export {exportIds.length}
            </button>
          </>
        )}

        {update && (
          <button
            className="btn update-chip"
            title={update.notes || `Version ${update.latest} is available`}
            onClick={() => {
              openUrl(update.url).catch(() => {});
              setUpdate(null);
            }}
          >
            Update to {update.latest}
          </button>
        )}

        <button className="btn ghost" onClick={() => setShowHelp(true)} title="Shortcuts">
          ?
        </button>

        {items.length > 0 && (
          <button
            className={`btn ghost panel-toggle ${rightOpen ? "on" : ""}`}
            onClick={() => setRightOpen((v) => !v)}
            title={rightOpen ? "Hide the detail panel" : "Show the detail panel"}
          >
            ▤
          </button>
        )}
      </header>

      <div className="body">
        <Sidebar
          settings={settings}
          onChange={applySettings}
          onReset={() => applySettings(DEFAULT_SETTINGS)}
          cacheBytes={cacheBytes}
          onPurgeCache={async () => {
            await api.purgeCache().catch(() => {});
            refreshCache();
          }}
          busy={busy}
          counts={{
            keep: counts.keep,
            maybe: counts.maybe,
            reject: counts.reject,
            total: counts.all,
          }}
          collapsed={!leftOpen}
          scenery={summary?.scenery ?? false}
        />

        {items.length === 0 ? (
          <div className="grid-wrap">
            <div className="empty">
              <h2>{busy ? "Analysing…" : "No folder open"}</h2>
              <p>
                {busy
                  ? "Reading each frame once at reduced resolution, then measuring focus, exposure, faces and near-duplicates."
                  : "Open a folder of JPEGs. Every frame gets scored for sharpness, exposure and eye state, bursts are grouped so you only judge the best of each, and your picks resolve back to the matching RAW files on export."}
              </p>
              {!busy && (
                <div style={{ display: "flex", gap: 8, justifyContent: "center", marginTop: 6 }}>
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={recursive}
                      onChange={(e) => setRecursive(e.target.checked)}
                    />
                    <span>Include subfolders</span>
                  </label>
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={detectSubjects}
                      onChange={(e) => setDetectSubjects(e.target.checked)}
                    />
                    <span>Find subjects</span>
                  </label>
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={detectFaces}
                      onChange={(e) => setDetectFaces(e.target.checked)}
                    />
                    <span>Check eyes</span>
                  </label>
                </div>
              )}
              {!busy && (
                <>
                  <button className="btn primary" onClick={chooseFolder}>
                    Open folder…
                  </button>
                  <div
                    className="path-row"
                    style={{ maxWidth: 520, margin: "4px auto 0" }}
                  >
                    <input
                      type="text"
                      value={manualPath}
                      placeholder="…or paste a folder path and press Enter"
                      onChange={(e) => setManualPath(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && openTypedPath()}
                    />
                    <button
                      className="btn"
                      onClick={openTypedPath}
                      disabled={!manualPath.trim()}
                    >
                      Open
                    </button>
                  </div>
                </>
              )}
            </div>
          </div>
        ) : (
          <PhotoGrid
            entries={entries}
            selectedId={selectedId}
            selection={selection}
            cardSize={cardSize}
            onSelect={handleSelect}
            onToggleStack={toggleStack}
            showBoxes={showBoxes}
            onOpen={(id) => {
              const i = view.findIndex((a) => a.meta.id === id);
              if (i >= 0) setLoupeIndex(i);
            }}
          />
        )}

        <DetailPanel
          item={selected}
          showBoxes={showBoxes}
          collapsed={!rightOpen}
          onPick={pick}
          onRate={rate}
          onOpenLoupe={() => {
            const i = view.findIndex((a) => a.meta.id === selectedId);
            if (i >= 0) setLoupeIndex(i);
          }}
        />
      </div>

      <footer className="statusbar">
        {busy && progress ? (
          <>
            <span className="spin" />
            <span>
              {progress.done} / {progress.total} · {progress.current}
            </span>
            <div className="progress">
              <span style={{ width: `${pct}%` }} />
            </div>
            <span>
              {progress.etaMs !== null
                ? `about ${Math.ceil(progress.etaMs / 1000)}s left`
                : "estimating…"}
            </span>
          </>
        ) : summary ? (
          <>
            <span>
              <i className="dot" style={{ background: "var(--keep)" }} />
              {summary.keep} keep
            </span>
            <span>
              <i className="dot" style={{ background: "var(--maybe)" }} />
              {summary.maybe} maybe
            </span>
            <span>
              <i className="dot" style={{ background: "var(--reject)" }} />
              {summary.reject} reject
            </span>
            <span>{summary.groups} bursts</span>
            {summary.durationMs > 0 && (
              <span>analysed in {(summary.durationMs / 1000).toFixed(1)}s</span>
            )}
            <div className="spacer" />
            {selection.size > 1 && <span>{selection.size} selected</span>}
            <button className="btn sm ghost" onClick={() => bulkPick("keep")}>
              Mark keep
            </button>
            <button className="btn sm ghost" onClick={() => bulkPick("reject")}>
              Mark reject
            </button>
            <button className="btn sm ghost" onClick={() => bulkPick(null)}>
              Clear marks
            </button>
            <button
              className="btn sm ghost"
              onClick={async () => {
                const f = await save({
                  title: "Save analysis as CSV",
                  defaultPath: "culling-report.csv",
                  filters: [{ name: "CSV", extensions: ["csv"] }],
                });
                if (typeof f === "string") await api.exportCsv(f).catch((e) => setError(String(e)));
              }}
            >
              CSV
            </button>
          </>
        ) : (
          <span>Ready</span>
        )}
        {error && (
          <span style={{ color: "var(--reject)", marginLeft: "auto" }} title={error}>
            {error.slice(0, 120)}
            <button className="btn sm ghost" onClick={() => setError(null)}>
              ✕
            </button>
          </span>
        )}
      </footer>

      {loupeIndex !== null && (
        <Loupe
          items={view}
          index={loupeIndex}
          groupMembers={groupMembers}
          onIndex={(i) => {
            setLoupeIndex(i);
            setSelectedId(view[i]?.meta.id ?? null);
          }}
          onPick={pick}
          showBoxes={showBoxes}
          onToggleBoxes={() => setShowBoxes((v) => !v)}
          onClose={() => setLoupeIndex(null)}
        />
      )}

      {showExport && (
        <ExportDialog selectedIds={exportIds} onClose={() => setShowExport(false)} />
      )}

      {showCloud && (
        <CloudDialog
          config={cloudCfg}
          onSaved={setCloudCfg}
          onDone={async () => setItems(await api.getItems())}
          onClose={() => setShowCloud(false)}
        />
      )}

      {showHelp && (
        <div className="overlay" onClick={() => setShowHelp(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>Shortcuts</h2>
            <p className="lede">
              Culling is faster from the keyboard. Marks always override the
              computed verdict.
            </p>
            <div className="help-grid">
              <span className="kbd">← → ↑ ↓</span>
              <span>Move between photos</span>
              <span className="kbd">Space</span>
              <span>Open the loupe</span>
              <span className="kbd">C</span>
              <span>Compare the burst side by side (in the loupe)</span>
              <span className="kbd">P</span>
              <span>Mark as keep</span>
              <span className="kbd">X</span>
              <span>Mark as reject</span>
              <span className="kbd">U</span>
              <span>Clear the mark</span>
              <span className="kbd">0–5</span>
              <span>Star rating, written into XMP sidecars</span>
              <span className="kbd">Esc</span>
              <span>Close the loupe</span>
            </div>
            <div className="modal-actions">
              <button className="btn primary" onClick={() => setShowHelp(false)}>
                Got it
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

