// 感情 tab: live visualization of the affect-drive model (22 neurotransmitter-
// analogue parameters recomputed every conversation turn) plus the memory
// module's short-term summary and long-term store.
//
// The drive/familiarity/badges data rides the existing WS connection
// (useNpcSocket's `affect`/`affectHistory`) — nothing to fetch here for that
// half. The memory panel is REST-backed (GET /api/memory): it loads on
// mount and again whenever `memoryVersion` increments, which useNpcSocket
// bumps on every `memory` WS frame, so the panel refreshes itself without
// the operator having to do anything. A manual reload button covers the
// case where memory changed for a reason that doesn't emit that frame.
import { useEffect, useMemo, useState } from "preact/hooks";
import {
  Activity,
  AlertTriangle,
  Brain,
  Clock,
  Database,
  Gauge,
  RefreshCw,
  ShieldAlert,
  Sparkles,
} from "lucide-preact";

import { getMemory } from "../lib/api";
import type { MemoryDocument } from "../lib/types";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import { useI18n } from "../hooks/useI18n";
import type { MessageKey, Translate } from "../lib/i18n";
import "../styles/components.css";
import "../styles/brain.css";

/** Fixed display order for the 22 drives — matches the WS frame's own fixed
 *  order (see lib/types.ts's AffectMessage doc comment). Kept as an explicit
 *  list (rather than derived from the first frame) so the trend dropdown and
 *  legend have something to render before any frame has arrived. */
const DRIVE_ORDER = [
  "dopamine",
  "serotonin",
  "oxytocin",
  "endorphin",
  "cortisol",
  "noradrenaline",
  "adrenaline",
  "acetylcholine",
  "glutamate",
  "gaba",
  "glycine",
  "melatonin",
  "orexin",
  "histamine",
  "dynorphin",
  "dhea",
  "enkephalin",
  "anandamide",
  "substance_p",
  "npy",
  "cck",
  "bdnf",
] as const;

const DRIVE_LABEL_KEYS: Record<string, MessageKey> = {
  dopamine: "brain.drive.dopamine",
  serotonin: "brain.drive.serotonin",
  oxytocin: "brain.drive.oxytocin",
  endorphin: "brain.drive.endorphin",
  cortisol: "brain.drive.cortisol",
  noradrenaline: "brain.drive.noradrenaline",
  adrenaline: "brain.drive.adrenaline",
  acetylcholine: "brain.drive.acetylcholine",
  glutamate: "brain.drive.glutamate",
  gaba: "brain.drive.gaba",
  glycine: "brain.drive.glycine",
  melatonin: "brain.drive.melatonin",
  orexin: "brain.drive.orexin",
  histamine: "brain.drive.histamine",
  dynorphin: "brain.drive.dynorphin",
  dhea: "brain.drive.dhea",
  enkephalin: "brain.drive.enkephalin",
  anandamide: "brain.drive.anandamide",
  substance_p: "brain.drive.substance_p",
  npy: "brain.drive.npy",
  cck: "brain.drive.cck",
  bdnf: "brain.drive.bdnf",
};

// The affect engine folds the 3 drives with |level - base| >= this threshold
// into the persona prompt each turn — see the task brief; kept in sync by
// convention with the Rust side rather than sent over the wire.
const DEVIATION_THRESHOLD = 0.2;
const FAMILIARITY_WARY_THRESHOLD = 0.4;

function driveLabel(t: Translate, key: string): string {
  const msgKey = DRIVE_LABEL_KEYS[key];
  return msgKey ? t(msgKey) : key;
}

function clampPct(v: number): number {
  return Math.round(Math.max(0, Math.min(1, v)) * 100);
}

function relativeTime(iso: string, t: Translate): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return iso;
  const diffSec = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (diffSec < 60) return t("brain.memory.time.justNow");
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return t("brain.memory.time.minutesAgo", { n: diffMin });
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return t("brain.memory.time.hoursAgo", { n: diffHour });
  const diffDay = Math.floor(diffHour / 24);
  return t("brain.memory.time.daysAgo", { n: diffDay });
}

function DriveBar({
  driveKey,
  level,
  base,
  rank,
  active,
  t,
}: {
  driveKey: string;
  level: number;
  base: number;
  rank: number | null;
  active: boolean;
  t: Translate;
}) {
  const label = driveLabel(t, driveKey);
  const levelPct = clampPct(level);
  const basePct = clampPct(base);
  const delta = level - base;
  const deltaPct = Math.round(delta * 100);
  const direction = delta > 0.0005 ? "up" : delta < -0.0005 ? "down" : "flat";
  const fillLeft = Math.min(level, base) * 100;
  const fillWidth = Math.abs(level - base) * 100;

  return (
    <div
      class={`brain-drive${active ? " brain-drive--active" : ""}${rank !== null ? " brain-drive--top" : ""}`}
    >
      <div class="brain-drive-head">
        <span class="brain-drive-name">{label}</span>
        {rank !== null ? (
          <span class="badge brain-drive-badge brain-drive-badge--top">
            {t("brain.drive.badge.top", { n: rank })}
          </span>
        ) : (
          active && (
            <span class="badge brain-drive-badge brain-drive-badge--active">{t("brain.drive.badge.active")}</span>
          )
        )}
        <span class={`brain-drive-delta brain-drive-delta--${direction}`}>
          {delta > 0 ? "+" : ""}
          {deltaPct}%
        </span>
      </div>
      <div
        class="brain-drive-bar"
        role="meter"
        aria-valuenow={levelPct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={t("brain.drive.aria", { name: label, level: levelPct, base: basePct })}
      >
        <div class="brain-drive-bar-track">
          <div
            class={`brain-drive-bar-fill brain-drive-bar-fill--${direction}`}
            style={{ left: `${fillLeft}%`, width: `${fillWidth}%` }}
          />
          <div class="brain-drive-bar-base" style={{ left: `${basePct}%` }} />
        </div>
      </div>
      <div class="brain-drive-foot">
        <span class="brain-drive-level">{levelPct}%</span>
        <span class="brain-drive-base-label">{t("brain.drive.baseLabel", { value: basePct })}</span>
      </div>
    </div>
  );
}

function FamiliarityGauge({ value, t }: { value: number; t: Translate }) {
  const pct = clampPct(value);
  const wary = value < FAMILIARITY_WARY_THRESHOLD;
  return (
    <div class="brain-familiarity">
      <div class="brain-familiarity-head">
        <Gauge size={16} aria-hidden="true" />
        <span>{t("brain.familiarity.label")}</span>
        <span class="brain-familiarity-value">{pct}%</span>
        {wary && <span class="badge brain-familiarity-wary">{t("brain.familiarity.wary")}</span>}
      </div>
      <div
        class="brain-familiarity-bar"
        role="meter"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={t("brain.familiarity.label")}
      >
        <div class="brain-familiarity-bar-track">
          <div class="brain-familiarity-bar-fill" style={{ width: `${pct}%` }} />
          <div
            class="brain-familiarity-bar-threshold"
            style={{ left: `${FAMILIARITY_WARY_THRESHOLD * 100}%` }}
          />
        </div>
      </div>
    </div>
  );
}

/** Dependency-free trend line: a plain `<polyline>` over a fixed viewBox, so
 *  it scales with its container without any charting library. */
function Sparkline({ points }: { points: number[] }) {
  const width = 300;
  const height = 64;
  if (points.length < 2) return null;
  const step = width / (points.length - 1);
  const coords = points
    .map((v, i) => `${(i * step).toFixed(1)},${(height - Math.max(0, Math.min(1, v)) * height).toFixed(1)}`)
    .join(" ");
  return (
    <svg
      class="brain-sparkline-svg"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <polyline points={coords} class="brain-sparkline-line" />
    </svg>
  );
}

export interface BrainViewProps {
  affect: AffectSnapshot | null;
  affectHistory: AffectSnapshot[];
  memoryVersion: number;
}

export function BrainView({ affect, affectHistory, memoryVersion }: BrainViewProps) {
  const { t } = useI18n();
  const [memory, setMemory] = useState<MemoryDocument | null>(null);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [memoryLoading, setMemoryLoading] = useState(false);
  const [selectedDrive, setSelectedDrive] = useState<string | null>(null);

  function loadMemory() {
    setMemoryLoading(true);
    setMemoryError(null);
    getMemory()
      .then((doc) => setMemory(doc))
      .catch((err) => setMemoryError(err instanceof Error ? err.message : String(err)))
      .finally(() => setMemoryLoading(false));
  }

  // Initial load, then re-fetch every time a `memory` WS frame lands.
  useEffect(() => {
    loadMemory();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [memoryVersion]);

  // Default the trend picker to whichever drive is currently deviating the
  // most, but only once — once the operator (or this default) has picked a
  // drive, later frames must not yank the selection out from under them.
  useEffect(() => {
    if (!affect) return;
    setSelectedDrive((current) => {
      if (current && affect.drives.some((d) => d.key === current)) return current;
      const top = affect.drives.slice().sort((a, b) => Math.abs(b.level - b.base) - Math.abs(a.level - a.base))[0];
      return top?.key ?? current;
    });
  }, [affect]);

  const ranked = useMemo(() => {
    const activeKeys = new Set<string>();
    const topKeys = new Map<string, number>();
    if (!affect) return { activeKeys, topKeys };
    const withDev = affect.drives
      .map((d) => ({ key: d.key, dev: Math.abs(d.level - d.base) }))
      .filter((d) => d.dev >= DEVIATION_THRESHOLD)
      .sort((a, b) => b.dev - a.dev);
    for (const d of withDev) activeKeys.add(d.key);
    withDev.slice(0, 3).forEach((d, i) => topKeys.set(d.key, i + 1));
    return { activeKeys, topKeys };
  }, [affect]);

  const trendHistory = useMemo(() => {
    if (!selectedDrive) return [];
    return affectHistory.map((snap) => snap.drives.find((d) => d.key === selectedDrive)?.level ?? 0);
  }, [affectHistory, selectedDrive]);

  const longTermSorted = useMemo(() => {
    if (!memory) return [];
    return memory.longTerm
      .slice()
      .sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime());
  }, [memory]);

  return (
    <div class="brain-view">
      <section class="brain-section">
        <h2 class="brain-section-title">
          <Activity size={16} aria-hidden="true" />
          {t("brain.status.title")}
        </h2>
        {affect ? (
          <>
            <FamiliarityGauge value={affect.familiarity} t={t} />
            {(affect.closing || affect.inviteCaution) && (
              <div class="brain-status-badges">
                {affect.closing && (
                  <span class="badge brain-status-badge">
                    <Clock size={12} aria-hidden="true" />
                    {t("brain.badge.closing")}
                  </span>
                )}
                {affect.inviteCaution && (
                  <span class="badge brain-status-badge brain-status-badge--caution">
                    <ShieldAlert size={12} aria-hidden="true" />
                    {t("brain.badge.inviteCaution")}
                  </span>
                )}
              </div>
            )}
          </>
        ) : (
          <div class="empty-state">
            <div class="empty-state-icon">
              <Brain size={24} />
            </div>
            <div class="empty-state-title">{t("brain.empty.title")}</div>
            <div class="empty-state-description">{t("brain.empty.desc")}</div>
          </div>
        )}
      </section>

      {affect && (
        <>
          <section class="brain-section">
            <div class="brain-section-head">
              <h2 class="brain-section-title">
                <Brain size={16} aria-hidden="true" />
                {t("brain.drives.title")}
              </h2>
              <div class="brain-drives-legend">
                <span class="brain-drives-legend-item brain-drives-legend-item--up">
                  {t("brain.drives.legend.above")}
                </span>
                <span class="brain-drives-legend-item brain-drives-legend-item--down">
                  {t("brain.drives.legend.below")}
                </span>
              </div>
            </div>
            <p class="brain-hint">{t("brain.drives.hint")}</p>
            <div class="brain-drive-grid">
              {affect.drives.map((d) => (
                <DriveBar
                  key={d.key}
                  driveKey={d.key}
                  level={d.level}
                  base={d.base}
                  rank={ranked.topKeys.get(d.key) ?? null}
                  active={ranked.activeKeys.has(d.key)}
                  t={t}
                />
              ))}
            </div>
          </section>

          <section class="brain-section">
            <h2 class="brain-section-title">
              <Sparkles size={16} aria-hidden="true" />
              {t("brain.trend.title")}
            </h2>
            <label class="brain-trend-select">
              <span>{t("brain.trend.select")}</span>
              <select
                value={selectedDrive ?? ""}
                onChange={(e) => setSelectedDrive((e.target as HTMLSelectElement).value)}
              >
                {DRIVE_ORDER.map((key) => (
                  <option key={key} value={key}>
                    {driveLabel(t, key)}
                  </option>
                ))}
              </select>
            </label>
            {trendHistory.length < 2 ? (
              <div class="empty-state">
                <div class="empty-state-title">{t("brain.trend.empty")}</div>
              </div>
            ) : (
              <div class="brain-sparkline">
                <Sparkline points={trendHistory} />
                <span class="brain-sparkline-current">
                  {clampPct(trendHistory[trendHistory.length - 1] ?? 0)}%
                </span>
              </div>
            )}
          </section>
        </>
      )}

      <section class="brain-section">
        <div class="brain-section-head">
          <h2 class="brain-section-title">
            <Database size={16} aria-hidden="true" />
            {t("brain.memory.title")}
          </h2>
          <button type="button" class="btn btn-ghost btn-small" onClick={() => loadMemory()} disabled={memoryLoading}>
            <RefreshCw size={13} class={memoryLoading ? "spin" : undefined} aria-hidden="true" />
            {t("common.reload")}
          </button>
        </div>

        {memoryError ? (
          <div class="empty-state">
            <div class="empty-state-icon">
              <AlertTriangle size={24} />
            </div>
            <div class="empty-state-title">{t("brain.memory.loadError")}</div>
            <div class="empty-state-description">{memoryError}</div>
          </div>
        ) : (
          <div class="brain-memory-grid">
            <div class="brain-memory-card">
              <h3 class="brain-memory-card-title">{t("brain.memory.shortTerm.title")}</h3>
              {memory && memory.shortTerm.trim() ? (
                <p class="brain-memory-short-text">{memory.shortTerm}</p>
              ) : (
                <p class="brain-memory-empty">{t("brain.memory.shortTerm.empty")}</p>
              )}
            </div>

            <div class="brain-memory-card">
              <div class="brain-memory-card-head">
                <h3 class="brain-memory-card-title">{t("brain.memory.longTerm.title")}</h3>
                {memory && (
                  <span class="chip">{t("brain.memory.longTerm.count", { count: memory.longTerm.length })}</span>
                )}
              </div>
              {longTermSorted.length > 0 ? (
                <ul class="brain-memory-long-list">
                  {longTermSorted.map((doc) => (
                    <li key={doc.docId} class="brain-memory-long-item">
                      <p class="brain-memory-long-text">{doc.text}</p>
                      <time class="brain-memory-long-time">{relativeTime(doc.createdAt, t)}</time>
                    </li>
                  ))}
                </ul>
              ) : (
                <p class="brain-memory-empty">{t("brain.memory.longTerm.empty")}</p>
              )}
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
