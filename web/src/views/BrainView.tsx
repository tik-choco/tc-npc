// 感情 tab: live visualization of the affect-drive model (22 neurotransmitter-
// analogue parameters recomputed every conversation turn) plus the memory
// module's short-term summary and long-term store.
//
// The current drive/familiarity/badges snapshot rides the existing WS
// connection (useNpcSocket's `affect`) — nothing to fetch for that half. The
// trend sparkline's *history*, though, is backed by both a live source and a
// REST one: useNpcSocket's `affectHistory` only ever holds frames that
// arrived while this tab's socket was open, so it starts empty on every page
// load — the one moment an operator most wants to see what the NPC has been
// feeling. GET /api/affect/history backfills the server's own rolling buffer
// on mount to cover exactly that gap; see `combinedAffectHistory` below for
// how the two are merged without double-counting a snapshot that lands in
// both. The memory panel is REST-backed too (GET /api/memory): it loads on
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
  Repeat,
  ShieldAlert,
  Sparkles,
  UserRound,
} from "lucide-preact";

import { Markdown } from "../components/Markdown";
import { getAffectHistory, getMemory } from "../lib/api";
import type { MemoryDocument } from "../lib/types";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import { useI18n } from "../hooks/useI18n";
import type { Translate } from "../lib/i18n";
// Drive order/labels/thresholds and the deviation ranking are shared with the
// チャット tab's condensed 内心 block — see lib/affect.ts.
import {
  DRIVE_ORDER,
  FAMILIARITY_WARY_THRESHOLD,
  PROMPT_DRIVE_COUNT,
  clampPct,
  deltaDirection,
  driveLabel,
  rankDeviations,
} from "../lib/affect";
import "../styles/components.css";
import "../styles/brain.css";

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
  const direction = deltaDirection(delta);
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
  // Server-side backfill for the trend sparkline, fetched once on mount.
  // Empty until the fetch resolves and stays empty forever if it fails —
  // either way `combinedAffectHistory` below just falls through to
  // `affectHistory` alone, exactly like before this backfill existed.
  const [affectBackfill, setAffectBackfill] = useState<AffectSnapshot[]>([]);

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

  // One-shot backfill of the trend history, fetched once when the tab first
  // mounts. Deliberately not re-fetched on later renders/reconnects: from
  // here on, live `affect` WS frames (the `affectHistory` prop) carry the
  // conversation forward, and re-fetching would just re-request the same
  // tail this session has already been receiving live.
  useEffect(() => {
    let cancelled = false;
    getAffectHistory()
      .then(({ entries }) => {
        if (cancelled) return;
        setAffectBackfill(
          entries.map((e) => ({
            ts: e.ts,
            familiarity: e.familiarity,
            closing: e.closing,
            inviteCaution: e.inviteCaution,
            // Normalized to null so this matches AffectSnapshot's own
            // convention (set in useNpcSocket's `affect` handler) of one
            // "nobody named" value instead of undefined-vs-null.
            partner: e.partner ?? null,
            partnerKnown: e.partnerKnown,
            partnerSwitched: e.partnerSwitched,
            partnerAway: e.partnerAway,
            drives: e.drives,
          })),
        );
      })
      .catch((err) => {
        // A missing/failed history must never blank the tab: leave
        // `affectBackfill` at its initial empty array so the sparkline
        // simply falls back to whatever `affectHistory` (live) already has,
        // exactly as it behaved before this backfill existed.
        console.error("failed to load affect history", err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
    const withDev = rankDeviations(affect);
    for (const d of withDev) activeKeys.add(d.key);
    withDev.slice(0, PROMPT_DRIVE_COUNT).forEach((d, i) => topKeys.set(d.key, i + 1));
    return { activeKeys, topKeys };
  }, [affect]);

  // Merge the one-shot server backfill with the live WS history without
  // mutating the `affectHistory` prop (owned by useNpcSocket) and without
  // double-counting a snapshot that shows up in both — which happens
  // whenever a turn fires between the history GET going out and it
  // resolving: that snapshot is already in `affectHistory` by the time the
  // backfill arrives (see bus_forward.rs: the buffer is updated before the
  // WS broadcast, so the reverse race — present in the buffer but not yet
  // broadcast — can't happen the other way around). `ts` is unique per
  // snapshot (bus_forward.rs gives the buffered copy and the broadcast frame
  // the exact same `ts`), so exact equality is enough to recognize the
  // overlap; no fuzzy matching needed.
  const combinedAffectHistory = useMemo(() => {
    if (affectBackfill.length === 0) return affectHistory;
    const liveTimestamps = new Set(affectHistory.map((snap) => snap.ts));
    const merged = [...affectBackfill.filter((snap) => !liveTimestamps.has(snap.ts)), ...affectHistory];
    merged.sort((a, b) => a.ts - b.ts);
    return merged;
  }, [affectBackfill, affectHistory]);

  const trendHistory = useMemo(() => {
    if (!selectedDrive) return [];
    return combinedAffectHistory.map((snap) => snap.drives.find((d) => d.key === selectedDrive)?.level ?? 0);
  }, [combinedAffectHistory, selectedDrive]);

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
            {/* Whose state this is. The drive model is per conversation
                partner (npc-talk's PartnerAffect), so every number below is
                scoped to this person — a familiarity that reset is a
                different partner, not a lost memory. */}
            <div class={`brain-partner${affect.partnerAway ? " is-away" : ""}`}>
              <UserRound size={16} aria-hidden="true" />
              <span class="brain-partner-label">{t("brain.partner.label")}</span>
              <span class={`brain-partner-name${affect.partner === null ? " is-unknown" : ""}`}>
                {affect.partner ?? t("brain.partner.unknown")}
              </span>
              {affect.partnerAway ? (
                <span class="badge brain-partner-badge brain-partner-badge--away">
                  <Clock size={12} aria-hidden="true" />
                  {t("brain.partner.away")}
                </span>
              ) : affect.partnerSwitched ? (
                <span class="badge brain-partner-badge brain-partner-badge--switched">
                  <Repeat size={12} aria-hidden="true" />
                  {t("brain.partner.switched")}
                </span>
              ) : (
                affect.partner !== null && (
                  <span class="badge brain-partner-badge">
                    {affect.partnerKnown ? t("brain.partner.known") : t("brain.partner.first")}
                  </span>
                )
              )}
            </div>
            {affect.partnerAway && <p class="brain-hint">{t("brain.partner.away.hint")}</p>}
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
                // Same model-written Markdown the チャット transcript renders
                // (see ChatView's MemoryLine) — these are literally the same
                // documents, so they can't show `**` as asterisks in one tab
                // and as emphasis in the other. Nothing here is clamped, so
                // both panels get the full block layout.
                <Markdown text={memory.shortTerm} class="brain-memory-short-text" />
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
                      <Markdown text={doc.text} class="brain-memory-long-text" />
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
