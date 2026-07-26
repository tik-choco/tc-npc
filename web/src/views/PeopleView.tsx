// 人物 tab: roster of everyone the NPC has recognized via chat, vision or
// scheduled events, plus a detail panel to inspect/edit one of them.
//
// The list is REST-backed (GET /api/people): it loads on mount and again
// whenever `peopleVersion` increments (bumped by useNpcSocket on every
// `person`/`personDeleted` WS frame — the same "refetch on version bump"
// idea BrainView uses for GET /api/memory). Between refetches, the live
// `people` WS prop is merged on top by id so a brand-new person or an
// updated familiarity/fact shows up immediately without waiting on the
// network round-trip; deletions by another client are picked up on the
// next version-triggered refetch rather than instantly, which is an
// acceptable lag for an operator console.
//
// The detail panel is its own REST fetch (GET /api/people/:id) keyed on the
// selected id, refreshed on the same `peopleVersion` bump but without
// clearing the current view first (stale-while-revalidate) — only actually
// switching the selection shows a loading state. Edits to name/aliases/notes
// use the same local-draft-until-blur pattern as ScheduleView's rows.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { AlertTriangle, Gauge, Loader2, RefreshCw, Trash2, UserPlus, UserRound } from "lucide-preact";

import { createPerson, deletePerson, getPeople, getPerson, updatePerson } from "../lib/api";
import type { PersonDetail, PersonRecord } from "../lib/types";
import { SaveChip } from "../components/SaveChip";
import { Toast, type ToastState } from "../components/Toast";
import { useI18n } from "../hooks/useI18n";
import type { MessageKey, Translate } from "../lib/i18n";
import type { SaveState } from "../hooks/useConfigDoc";
import "../styles/components.css";
import "../styles/people.css";

/** How long a successful save's chip / toast stays up before clearing. */
const SAVED_INDICATOR_MS = 2500;
const TOAST_MS = 4000;

const SOURCE_LABEL_KEYS: Record<string, MessageKey> = {
  chat: "people.source.chat",
  vision: "people.source.vision",
  event: "people.source.event",
  manual: "people.source.manual",
};

function sourceLabel(t: Translate, source: string): string {
  const key = SOURCE_LABEL_KEYS[source];
  return key ? t(key) : source;
}

function clampPct(v: number): number {
  return Math.round(Math.max(0, Math.min(1, v)) * 100);
}

/** Shared "N分前" logic for both the unix-second timestamps on PersonRecord
 *  (first/last seen, fact.createdAt) and the ISO timestamps that come back
 *  from the long-term memory store (PersonDetail.memories[].createdAt). Kept
 *  local rather than imported from BrainView, which owns no exports. */
function relativeTimeFromMs(ms: number, t: Translate): string {
  if (Number.isNaN(ms)) return "";
  const diffSec = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (diffSec < 60) return t("brain.memory.time.justNow");
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return t("brain.memory.time.minutesAgo", { n: diffMin });
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return t("brain.memory.time.hoursAgo", { n: diffHour });
  const diffDay = Math.floor(diffHour / 24);
  return t("brain.memory.time.daysAgo", { n: diffDay });
}

function relativeTimeUnixSec(sec: number, t: Translate): string {
  if (!sec) return t("people.time.never");
  return relativeTimeFromMs(sec * 1000, t);
}

function relativeTimeIso(iso: string, t: Translate): string {
  return relativeTimeFromMs(new Date(iso).getTime(), t);
}

/** Lightweight familiarity meter — same look as BrainView's FamiliarityGauge,
 *  minus the "wary" threshold marker, since the 人物 tab cares about "how
 *  well is this one person known" rather than the live conversational gauge. */
function PersonFamiliarityMeter({ value, t }: { value: number; t: Translate }) {
  const pct = clampPct(value);
  return (
    <div class="people-familiarity">
      <div class="people-familiarity-head">
        <Gauge size={14} aria-hidden="true" />
        <span>{t("brain.familiarity.label")}</span>
        <span class="people-familiarity-value">{pct}%</span>
      </div>
      <div
        class="people-familiarity-bar-track"
        role="meter"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={t("brain.familiarity.label")}
      >
        <div class="people-familiarity-bar-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function SourceBadge({ source, t }: { source: string; t: Translate }) {
  return <span class={`badge people-source-badge people-source-badge--${source}`}>{sourceLabel(t, source)}</span>;
}

export interface PeopleViewProps {
  /** Live people set, upserted/removed by useNpcSocket from `person` /
   *  `personDeleted` WS frames. */
  people: PersonRecord[];
  /** Bumped on every such frame — triggers a GET /api/people (and, if a
   *  person is selected, a GET /api/people/:id) refetch. */
  peopleVersion: number;
}

export function PeopleView({ people, peopleVersion }: PeopleViewProps) {
  const { t } = useI18n();

  // --- Roster (REST + WS merge) -------------------------------------------
  const [restPeople, setRestPeople] = useState<PersonRecord[] | null>(null);
  const [listLoading, setListLoading] = useState(false);
  const [listError, setListError] = useState<string | null>(null);

  function loadPeopleList() {
    setListLoading(true);
    setListError(null);
    getPeople()
      .then((res) => setRestPeople(res.people))
      .catch((err) => setListError(err instanceof Error ? err.message : String(err)))
      .finally(() => setListLoading(false));
  }

  useEffect(() => {
    loadPeopleList();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [peopleVersion]);

  const merged = useMemo<PersonRecord[]>(() => {
    const map = new Map<string, PersonRecord>();
    for (const p of restPeople ?? []) map.set(p.id, p);
    for (const p of people) map.set(p.id, p);
    return Array.from(map.values()).sort((a, b) => b.lastSeen - a.lastSeen);
  }, [restPeople, people]);

  // --- Selection + detail --------------------------------------------------
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<PersonDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  function loadDetail(id: string) {
    setDetailError(null);
    getPerson(id)
      .then((d) => setDetail(d))
      .catch((err) => setDetailError(err instanceof Error ? err.message : String(err)));
  }

  // Switching the selection: clear the old detail so a stale person doesn't
  // flash while the new one loads.
  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      setDetailError(null);
      return;
    }
    setDetail(null);
    loadDetail(selectedId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId]);

  // A version bump for the currently-selected person: refresh in place
  // (stale-while-revalidate) rather than clearing the panel first.
  useEffect(() => {
    if (!selectedId) return;
    loadDetail(selectedId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [peopleVersion]);

  // Instant reflect of a `person` WS frame for whoever is currently open, so
  // a fact/appearance/familiarity update doesn't wait on the network fetch
  // the effect above kicks off.
  useEffect(() => {
    if (!selectedId) return;
    const live = people.find((p) => p.id === selectedId);
    if (!live) return;
    setDetail((d) => (d && d.person.id === selectedId ? { ...d, person: live } : d));
  }, [people, selectedId]);

  // --- Inline edit (name / aliases / notes) --------------------------------
  const [draftName, setDraftName] = useState("");
  const [draftAliases, setDraftAliases] = useState("");
  const [draftNotes, setDraftNotes] = useState("");
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Re-sync the drafts only when the selected person actually changes —
  // not on every WS-driven detail refresh, so mid-edit typing survives a
  // `person` frame for the same id (same reasoning as ScheduleView's rows).
  useEffect(() => {
    setDraftName(detail?.person.name ?? "");
    setDraftAliases(detail?.person.aliases.join(", ") ?? "");
    setDraftNotes(detail?.person.notes ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail?.person.id]);

  useEffect(
    () => () => {
      if (savedTimer.current !== null) clearTimeout(savedTimer.current);
    },
    [],
  );

  function saveField(patch: { name?: string; aliases?: string[]; notes?: string; appearance?: string }) {
    if (!detail) return;
    const id = detail.person.id;
    setSaveState("saving");
    setSaveError(null);
    updatePerson(id, patch)
      .then((res) => {
        setDetail((d) => (d && d.person.id === id ? { ...d, person: res.person } : d));
        setRestPeople((list) => (list ? list.map((p) => (p.id === id ? res.person : p)) : list));
        setSaveState("saved");
        if (savedTimer.current !== null) clearTimeout(savedTimer.current);
        savedTimer.current = setTimeout(() => setSaveState("idle"), SAVED_INDICATOR_MS);
      })
      .catch((err) => {
        setSaveError(err instanceof Error ? err.message : String(err));
        setSaveState("error");
      });
  }

  function commitName() {
    if (!detail || draftName === detail.person.name) return;
    saveField({ name: draftName });
  }

  function commitAliases() {
    if (!detail) return;
    const next = draftAliases
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s !== "");
    const prev = detail.person.aliases;
    if (next.length === prev.length && next.every((v, i) => v === prev[i])) return;
    saveField({ aliases: next });
  }

  function commitNotes() {
    if (!detail || draftNotes === detail.person.notes) return;
    saveField({ notes: draftNotes });
  }

  // --- Delete (confirm step required) --------------------------------------
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  useEffect(() => {
    setConfirmDeleteId(null);
  }, [selectedId]);

  async function confirmDelete() {
    if (!detail) return;
    const id = detail.person.id;
    setDeleteBusy(true);
    try {
      await deletePerson(id);
      setRestPeople((list) => (list ? list.filter((p) => p.id !== id) : list));
      setSelectedId((cur) => (cur === id ? null : cur));
      setConfirmDeleteId(null);
      setToast({ kind: "success", message: t("people.toast.deleted") });
    } catch (err) {
      setToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setDeleteBusy(false);
    }
  }

  // --- Add ------------------------------------------------------------------
  const [addName, setAddName] = useState("");
  const [addBusy, setAddBusy] = useState(false);
  const [toast, setToast] = useState<ToastState | null>(null);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), TOAST_MS);
    return () => clearTimeout(timer);
  }, [toast]);

  async function handleAdd() {
    const name = addName.trim();
    if (!name || addBusy) return;
    setAddBusy(true);
    try {
      const res = await createPerson({ name });
      setRestPeople((list) => (list ? [res.person, ...list] : [res.person]));
      setSelectedId(res.person.id);
      setAddName("");
      setToast({ kind: "success", message: t("people.toast.added", { name: res.person.name }) });
    } catch (err) {
      setToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setAddBusy(false);
    }
  }

  return (
    <div class="people-view">
      <div class="people-layout">
        <section class="people-list-panel">
          <div class="people-list-head">
            <h2 class="people-list-title">
              <UserRound size={16} aria-hidden="true" />
              {t("people.list.title")}
              {merged.length > 0 && <span class="chip">{t("people.list.count", { count: merged.length })}</span>}
            </h2>
            <button
              type="button"
              class="btn btn-ghost btn-small"
              onClick={() => loadPeopleList()}
              disabled={listLoading}
            >
              <RefreshCw size={13} class={listLoading ? "spin" : undefined} aria-hidden="true" />
              {t("common.reload")}
            </button>
          </div>

          <p class="people-hint">{t("people.hint")}</p>

          <div class="people-add-row">
            <input
              type="text"
              placeholder={t("people.add.placeholder")}
              aria-label={t("people.add.aria")}
              value={addName}
              disabled={addBusy}
              onInput={(e) => setAddName((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void handleAdd();
              }}
            />
            <button
              type="button"
              class="btn btn-ghost btn-small"
              disabled={addBusy || addName.trim() === ""}
              onClick={() => void handleAdd()}
            >
              {addBusy ? <Loader2 size={14} class="spin" aria-hidden="true" /> : <UserPlus size={14} aria-hidden="true" />}
              {t("people.add.button")}
            </button>
          </div>

          {listError ? (
            <div class="empty-state">
              <div class="empty-state-icon">
                <AlertTriangle size={24} />
              </div>
              <div class="empty-state-title">{t("people.list.loadError")}</div>
              <div class="empty-state-description">{listError}</div>
            </div>
          ) : merged.length === 0 ? (
            <div class="empty-state">
              {listLoading ? (
                <div class="empty-state-title">{t("common.loading")}</div>
              ) : (
                <>
                  <div class="empty-state-icon">
                    <UserRound size={24} />
                  </div>
                  <div class="empty-state-title">{t("people.list.empty.title")}</div>
                  <div class="empty-state-description">{t("people.list.empty.desc")}</div>
                </>
              )}
            </div>
          ) : (
            <ul class="people-list">
              {merged.map((p) => (
                <li key={p.id}>
                  <button
                    type="button"
                    class={`people-row${selectedId === p.id ? " people-row--active" : ""}`}
                    aria-pressed={selectedId === p.id}
                    onClick={() => setSelectedId(p.id)}
                  >
                    <span class="people-row-name">{p.name.trim() || t("people.row.unnamed")}</span>
                    <span class="people-row-meta">
                      <SourceBadge source={p.source} t={t} />
                      <span class="people-row-encounters">{t("people.row.encounters", { n: p.encounterCount })}</span>
                    </span>
                    <span class="people-row-time">{relativeTimeUnixSec(p.lastSeen, t)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section class="people-detail-panel">
          {!selectedId && (
            <div class="empty-state">
              <div class="empty-state-icon">
                <UserRound size={24} />
              </div>
              <div class="empty-state-title">{t("people.detail.empty.title")}</div>
              <div class="empty-state-description">{t("people.detail.empty.desc")}</div>
            </div>
          )}

          {selectedId && detailError && (
            <div class="empty-state">
              <div class="empty-state-icon">
                <AlertTriangle size={24} />
              </div>
              <div class="empty-state-title">{t("people.detail.loadError")}</div>
              <div class="empty-state-description">{detailError}</div>
            </div>
          )}

          {selectedId && !detailError && !detail && <div class="empty-state">{t("common.loading")}</div>}

          {selectedId && !detailError && detail && (
            <>
              <div class="people-detail-head">
                <div class="people-detail-name-row">
                  <input
                    type="text"
                    class="people-detail-name-input"
                    placeholder={t("people.detail.name.placeholder")}
                    value={draftName}
                    onInput={(e) => setDraftName((e.target as HTMLInputElement).value)}
                    onBlur={commitName}
                  />
                  <SourceBadge source={detail.person.source} t={t} />
                  <SaveChip state={saveState} error={saveError} />
                </div>

                {confirmDeleteId === detail.person.id ? (
                  <div class="people-delete-confirm">
                    <span class="people-delete-confirm-message">{t("people.delete.confirm.message")}</span>
                    <button
                      type="button"
                      class="btn btn-danger btn-small"
                      disabled={deleteBusy}
                      onClick={() => void confirmDelete()}
                    >
                      {deleteBusy ? <Loader2 size={13} class="spin" aria-hidden="true" /> : <Trash2 size={13} aria-hidden="true" />}
                      {t("people.delete.confirm.yes")}
                    </button>
                    <button
                      type="button"
                      class="btn btn-ghost btn-small"
                      disabled={deleteBusy}
                      onClick={() => setConfirmDeleteId(null)}
                    >
                      {t("common.cancel")}
                    </button>
                  </div>
                ) : (
                  <button
                    type="button"
                    class="icon-btn people-delete-btn"
                    title={t("people.delete.tooltip")}
                    aria-label={t("people.delete.tooltip")}
                    onClick={() => setConfirmDeleteId(detail.person.id)}
                  >
                    <Trash2 size={16} aria-hidden="true" />
                  </button>
                )}
              </div>

              <label class="field people-aliases-field">
                <span>{t("people.detail.aliasesLabel")}</span>
                <input
                  type="text"
                  placeholder={t("people.detail.aliases.placeholder")}
                  value={draftAliases}
                  onInput={(e) => setDraftAliases((e.target as HTMLInputElement).value)}
                  onBlur={commitAliases}
                />
                {detail.person.aliases.length > 0 && (
                  <div class="people-alias-chips">
                    {detail.person.aliases.map((alias) => (
                      <span key={alias} class="chip">
                        {alias}
                      </span>
                    ))}
                  </div>
                )}
              </label>

              <PersonFamiliarityMeter value={detail.person.familiarity} t={t} />

              <div class="people-stats-row">
                <div class="people-stat">
                  <span class="people-stat-label">{t("people.detail.firstSeen")}</span>
                  <span class="people-stat-value">{relativeTimeUnixSec(detail.person.firstSeen, t)}</span>
                </div>
                <div class="people-stat">
                  <span class="people-stat-label">{t("people.detail.lastSeen")}</span>
                  <span class="people-stat-value">{relativeTimeUnixSec(detail.person.lastSeen, t)}</span>
                </div>
                <div class="people-stat">
                  <span class="people-stat-label">{t("people.detail.encounterCount")}</span>
                  <span class="people-stat-value">{t("people.row.encounters", { n: detail.person.encounterCount })}</span>
                </div>
              </div>

              {detail.person.appearance.trim() !== "" && (
                <div class="people-section">
                  <h3 class="people-section-title">{t("people.detail.appearanceLabel")}</h3>
                  <p class="people-appearance-text">{detail.person.appearance}</p>
                </div>
              )}

              <label class="field people-notes-field">
                <span>{t("people.detail.notesLabel")}</span>
                <textarea
                  rows={3}
                  placeholder={t("people.detail.notes.placeholder")}
                  value={draftNotes}
                  onInput={(e) => setDraftNotes((e.target as HTMLTextAreaElement).value)}
                  onBlur={commitNotes}
                />
              </label>

              <div class="people-section">
                <h3 class="people-section-title">{t("people.detail.factsTitle")}</h3>
                {detail.person.facts.length === 0 ? (
                  <p class="people-empty-note">{t("people.detail.facts.empty")}</p>
                ) : (
                  <ul class="people-fact-list">
                    {detail.person.facts
                      .slice()
                      .sort((a, b) => b.createdAt - a.createdAt)
                      .map((fact, index) => (
                        <li key={index} class="people-fact-item">
                          <div class="people-fact-head">
                            <SourceBadge source={fact.source} t={t} />
                            <time class="people-fact-time">{relativeTimeUnixSec(fact.createdAt, t)}</time>
                          </div>
                          <p class="people-fact-text">{fact.text}</p>
                        </li>
                      ))}
                  </ul>
                )}
              </div>

              <div class="people-section">
                <h3 class="people-section-title">{t("people.detail.memoriesTitle")}</h3>
                {detail.memories.length === 0 ? (
                  <p class="people-empty-note">{t("people.detail.memories.empty")}</p>
                ) : (
                  <ul class="people-memory-list">
                    {detail.memories.map((mem) => (
                      <li key={mem.docId} class="people-memory-item">
                        <p class="people-memory-text">{mem.text}</p>
                        <time class="people-memory-time">{relativeTimeIso(mem.createdAt, t)}</time>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </>
          )}
        </section>
      </div>

      {toast && <Toast toast={toast} />}
    </div>
  );
}
