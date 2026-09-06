import { useEffect, useMemo, useRef, useState } from "react";
import { thumbUrl } from "../api";
import { coverFit, place } from "../fit";
import type { Analysis } from "../types";

/** One card in the grid, plus how it should be drawn as part of a stack. */
export interface GridEntry {
  item: Analysis;
  /** Frames represented by this card. >1 means a collapsed stack. */
  stackCount: number;
  /** True when this card is the head of an expanded stack. */
  expanded: boolean;
  /** True when this card is a member revealed by expanding a stack. */
  isChild: boolean;
}

interface Props {
  entries: GridEntry[];
  selectedId: string | null;
  cardSize: number;
  onSelect: (id: string, index: number, additive: boolean) => void;
  onOpen: (id: string) => void;
  onToggleStack: (groupId: number) => void;
  selection: Set<string>;
  /** Draw detected-subject boxes over the thumbnails. */
  showBoxes: boolean;
}

/**
 * Windowed photo grid.
 *
 * A shoot is routinely 800+ frames and each card holds an image, so mounting
 * them all costs hundreds of megabytes and makes scrolling stutter. We render
 * only the rows in view plus a small overscan, and position cards absolutely
 * inside a spacer sized to the full content height.
 */
export default function PhotoGrid({
  entries,
  selectedId,
  cardSize,
  onSelect,
  onOpen,
  onToggleStack,
  selection,
  showBoxes,
}: Props) {
  const items = entries;
  const wrapRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState({ w: 0, h: 0 });

  const GAP = 8;
  const PAD = 12;
  // 3:2 is the native aspect of every full-frame body; caption sits inside.
  const cardH = Math.round(cardSize / 1.5);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const measure = () =>
      setViewport({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const cols = Math.max(
    1,
    Math.floor((viewport.w - PAD * 2 + GAP) / (cardSize + GAP)),
  );
  const rows = Math.ceil(items.length / cols);
  const rowH = cardH + GAP;
  const totalH = rows * rowH + PAD * 2;

  // Overscan by two rows so fast scrolling does not reveal blank space.
  const firstRow = Math.max(0, Math.floor((scrollTop - PAD) / rowH) - 2);
  const lastRow = Math.min(
    rows,
    Math.ceil((scrollTop + viewport.h - PAD) / rowH) + 2,
  );

  const visible = useMemo(() => {
    const out: { entry: GridEntry; index: number; x: number; y: number }[] = [];
    for (let r = firstRow; r < lastRow; r++) {
      for (let c = 0; c < cols; c++) {
        const i = r * cols + c;
        if (i >= items.length) break;
        out.push({
          entry: items[i],
          index: i,
          x: PAD + c * (cardSize + GAP),
          y: PAD + r * rowH,
        });
      }
    }
    return out;
  }, [items, firstRow, lastRow, cols, cardSize, rowH]);

  // Keep the selected card on screen when the keyboard moves the cursor.
  useEffect(() => {
    if (!selectedId || !wrapRef.current) return;
    const idx = items.findIndex((e) => e.item.meta.id === selectedId);
    if (idx < 0) return;
    const row = Math.floor(idx / cols);
    const top = PAD + row * rowH;
    const el = wrapRef.current;
    if (top < el.scrollTop) el.scrollTo({ top: top - PAD });
    else if (top + cardH > el.scrollTop + el.clientHeight)
      el.scrollTo({ top: top + cardH - el.clientHeight + PAD });
  }, [selectedId, cols, rowH, cardH, items]);

  if (items.length === 0) {
    return (
      <div className="grid-wrap">
        <div className="empty">
          <h2>Nothing matches this filter</h2>
          <p>Try a different filter, or loosen the thresholds in the sidebar.</p>
        </div>
      </div>
    );
  }

  return (
    <div
      className="grid-wrap"
      ref={wrapRef}
      onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
    >
      <div className="grid-inner" style={{ height: totalH }}>
        {visible.map(({ entry, index, x, y }) => (
          <Card
            key={entry.item.meta.id}
            entry={entry}
            width={cardSize}
            height={cardH}
            x={x}
            y={y}
            selected={entry.item.meta.id === selectedId}
            multiSelected={selection.has(entry.item.meta.id)}
            onSelect={(additive) => onSelect(entry.item.meta.id, index, additive)}
            onOpen={() => onOpen(entry.item.meta.id)}
            onToggleStack={onToggleStack}
            showBoxes={showBoxes}
          />
        ))}
      </div>
    </div>
  );
}

interface CardProps {
  entry: GridEntry;
  width: number;
  height: number;
  x: number;
  y: number;
  selected: boolean;
  multiSelected: boolean;
  onSelect: (additive: boolean) => void;
  onOpen: () => void;
  onToggleStack: (groupId: number) => void;
  showBoxes: boolean;
}

/** Pick the two or three findings most worth showing on a thumbnail. */
function cardBadges(a: Analysis): { text: string; cls: string }[] {
  const out: { text: string; cls: string }[] = [];
  if (a.userPick === "keep") out.push({ text: "PICK", cls: "pick" });
  if (a.userPick === "reject") out.push({ text: "CUT", cls: "bad" });

  const priority = [
    "eyes-closed",
    "very-soft",
    "soft",
    "motion-blur",
    "blown",
    "duplicate",
  ];
  for (const code of priority) {
    const r = a.reasons.find((x) => x.code === code);
    if (!r) continue;
    const short: Record<string, string> = {
      "eyes-closed": "EYES",
      "very-soft": "BLUR",
      soft: "SOFT",
      "motion-blur": "MOTION",
      blown: "BLOWN",
      duplicate: "DUPE",
    };
    out.push({
      text: short[code] ?? code.toUpperCase(),
      cls: r.severity,
    });
    if (out.length >= 3) break;
  }
  return out;
}

function Card({
  entry,
  width,
  height,
  x,
  y,
  selected,
  multiSelected,
  onSelect,
  onOpen,
  onToggleStack,
  showBoxes,
}: CardProps) {
  const { item, stackCount, expanded, isChild } = entry;
  const isStack = stackCount > 1;

  const cls = [
    "card",
    selected || multiSelected ? "sel" : "",
    item.verdict === "reject" ? "rejected" : "",
    isStack ? "stack" : "",
    isChild ? "stack-child" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const badges = cardBadges(item);

  return (
    <div
      className={cls}
      style={{ left: x, top: y, width, height }}
      onClick={(e) => onSelect(e.ctrlKey || e.metaKey || e.shiftKey)}
      onDoubleClick={onOpen}
      title={
        isStack
          ? `${item.meta.fileName} — best of ${stackCount}. Click the badge to ${expanded ? "collapse" : "expand"}.`
          : item.meta.fileName
      }
    >
      {/* Offset layers behind the card, so a stack reads as depth at a glance. */}
      {isStack && !expanded && (
        <>
          <span className="stack-layer l2" />
          <span className="stack-layer l1" />
        </>
      )}
      <img
        className="thumb"
        src={thumbUrl(item.meta.id)}
        alt={item.meta.fileName}
        loading="lazy"
        draggable={false}
      />
      {showBoxes &&
        item.subjects
          // Only the classes being culled for, and only ones big enough to
          // read at thumbnail size — otherwise a wide shot becomes confetti.
          .filter((s) => s.counts && s.area > 0.0015)
          .slice(0, 10)
          .map((s, i) => (
            <div
              key={i}
              className="thumb-box"
              style={place(coverFit(item.meta, width / height), s)}
            />
          ))}
      <div className={`score ${item.verdict}`}>{Math.round(item.score)}</div>
      <div className="badges">
        {badges.map((b, i) => (
          <span key={i} className={`badge ${b.cls}`}>
            {b.text}
          </span>
        ))}
        {isStack ? (
          <button
            className="badge stack-toggle"
            title={expanded ? "Collapse this burst" : `Show all ${stackCount} frames`}
            onClick={(e) => {
              // Don't let the click fall through and re-select the card.
              e.stopPropagation();
              if (item.groupId !== null) onToggleStack(item.groupId);
            }}
          >
            {expanded ? "▾" : "▸"} {stackCount}
          </button>
        ) : (
          item.groupId !== null &&
          !isChild && (
            <span className={`badge ${item.isGroupBest ? "good" : ""}`}>
              {item.isGroupBest ? "★ BEST" : `#${item.groupRank + 1}`}
            </span>
          )
        )}
        {isChild && (
          <span className={`badge ${item.isGroupBest ? "good" : ""}`}>
            {item.isGroupBest ? "★ BEST" : `#${item.groupRank + 1}`}
          </span>
        )}
      </div>
      <div className="caption">{item.meta.fileName}</div>
      <div className={`verdict-bar verdict-${item.verdict}`} />
    </div>
  );
}

