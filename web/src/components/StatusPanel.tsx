// チャットサイドバーの「状態」パネル: renders the connection state, server
// version, active character, and enabled/disabled module set carried by the
// `hello`/`status` WS frames (see useNpcSocket) — previously received but
// never surfaced in the UI.
//
// It also carries a condensed 内心 card above that connection card. The full
// affect model lives in the 感情 tab (BrainView): all 22 drives, baseline
// markers, trend sparkline, memory store. That is the right place to study
// the model, but the wrong place to *read* it while a conversation is
// happening — switching tabs to answer "why did it just say that?" loses the
// transcript. So this card shows only what actually shapes the current
// reply: familiarity, the drives deviating far enough from baseline to be
// folded into the persona prompt (see PROMPT_DRIVE_COUNT), and the two
// conversational-mode flags. Everything else stays in the 感情 tab.
import { Activity, Brain, Clock, ShieldAlert, UserRound, Repeat } from "lucide-preact";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { ConnectionState } from "../lib/ws";
import type { Translate } from "../lib/i18n";
import {
  FAMILIARITY_WARY_THRESHOLD,
  PROMPT_DRIVE_COUNT,
  clampPct,
  deltaDirection,
  driveLabel,
  rankDeviations,
} from "../lib/affect";
import { useI18n } from "../hooks/useI18n";
import { ConnectionStatus } from "./ConnectionStatus";
import "../styles/components.css";
import "../styles/status.css";

export interface StatusPanelProps {
  connectionState: ConnectionState;
  version: string | null;
  character: { id: string; name: string } | null;
  modules: Record<string, boolean>;
  /** Latest affect frame, or null before the first one lands. */
  affect: AffectSnapshot | null;
}

/** Who these readings belong to. Sits above the numbers rather than beside
 *  them because the drive state is per partner (npc-talk's PartnerAffect) —
 *  without a name attached, a familiarity that drops to zero mid-session
 *  looks like the NPC forgetting rather than like a different person having
 *  started talking. `switched` stays set until the next turn lands, so the
 *  badge is still there while the operator reads the reply that caused it. */
function PartnerRow({
  partner,
  known,
  switched,
  away,
  t,
}: {
  partner: string | null;
  known: boolean;
  switched: boolean;
  away: boolean;
  t: Translate;
}) {
  return (
    <div class={`status-affect-partner${away ? " is-away" : ""}`}>
      <UserRound size={14} aria-hidden="true" />
      <span class="status-affect-partner-label">{t("brain.partner.label")}</span>
      <span class={`status-affect-partner-name${partner === null ? " is-unknown" : ""}`}>
        {partner ?? t("brain.partner.unknown")}
      </span>
      {/* One badge, most-current condition first: having gone quiet describes
          right now, a switch describes the turn that just happened, and
          known/first-meeting is the standing fact when neither applies. */}
      {away ? (
        <span class="badge status-affect-partner-badge status-affect-partner-badge--away">
          <Clock size={11} aria-hidden="true" />
          {t("brain.partner.away")}
        </span>
      ) : switched ? (
        <span class="badge status-affect-partner-badge status-affect-partner-badge--switched">
          <Repeat size={11} aria-hidden="true" />
          {t("brain.partner.switched")}
        </span>
      ) : (
        partner !== null && (
          <span class="badge status-affect-partner-badge">
            {known ? t("brain.partner.known") : t("brain.partner.first")}
          </span>
        )
      )}
    </div>
  );
}

/** Familiarity as a single slim bar with the wary threshold marked, so the
 *  reading is legible at a glance without the 感情 tab's larger gauge. */
function MiniFamiliarity({ value, t }: { value: number; t: Translate }) {
  const pct = clampPct(value);
  const wary = value < FAMILIARITY_WARY_THRESHOLD;
  return (
    <div class="status-affect-familiarity">
      <div class="status-affect-familiarity-head">
        <span class="status-affect-familiarity-label">{t("brain.familiarity.label")}</span>
        {wary && <span class="badge status-affect-wary">{t("brain.familiarity.wary")}</span>}
        <span class="status-affect-familiarity-value">{pct}%</span>
      </div>
      <div
        class="status-affect-bar"
        role="meter"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={t("brain.familiarity.label")}
      >
        <div class="status-affect-bar-track">
          <div class="status-affect-bar-fill" style={{ width: `${pct}%` }} />
          <div
            class="status-affect-bar-threshold"
            style={{ left: `${FAMILIARITY_WARY_THRESHOLD * 100}%` }}
          />
        </div>
      </div>
    </div>
  );
}

/** One deviating drive: name, signed delta, and a bar drawn from baseline to
 *  current level — the same left/width geometry as the 感情 tab's DriveBar,
 *  so the two readings stay visually comparable. */
function MiniDrive({
  driveKey,
  level,
  base,
  delta,
  t,
}: {
  driveKey: string;
  level: number;
  base: number;
  delta: number;
  t: Translate;
}) {
  const label = driveLabel(t, driveKey);
  const direction = deltaDirection(delta);
  const levelPct = clampPct(level);
  const basePct = clampPct(base);
  return (
    <li class="status-affect-drive">
      <div class="status-affect-drive-head">
        <span class="status-affect-drive-name">{label}</span>
        <span class={`status-affect-drive-delta status-affect-drive-delta--${direction}`}>
          {delta > 0 ? "+" : ""}
          {Math.round(delta * 100)}%
        </span>
      </div>
      <div
        class="status-affect-bar"
        role="meter"
        aria-valuenow={levelPct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={t("brain.drive.aria", { name: label, level: levelPct, base: basePct })}
      >
        <div class="status-affect-bar-track">
          <div
            class={`status-affect-bar-fill status-affect-bar-fill--${direction}`}
            style={{ left: `${Math.min(level, base) * 100}%`, width: `${Math.abs(delta) * 100}%` }}
          />
          <div class="status-affect-bar-base" style={{ left: `${basePct}%` }} />
        </div>
      </div>
    </li>
  );
}

function AffectCard({ affect, t }: { affect: AffectSnapshot | null; t: Translate }) {
  // Only the drives the engine actually folds into the prompt. When nothing
  // clears the threshold that is a meaningful reading in itself ("sitting at
  // baseline"), not an empty state, so it gets its own line rather than the
  // "no data yet" copy — which is reserved for having received no frame.
  const top = rankDeviations(affect).slice(0, PROMPT_DRIVE_COUNT);

  return (
    <section class="status-card">
      <div class="status-card-title">
        <Brain size={16} aria-hidden="true" />
        <span>{t("status.affect.title")}</span>
      </div>

      {affect === null ? (
        <p class="status-affect-empty">{t("status.affect.empty")}</p>
      ) : (
        <>
          <PartnerRow
            partner={affect.partner}
            known={affect.partnerKnown}
            switched={affect.partnerSwitched}
            away={affect.partnerAway}
            t={t}
          />
          <MiniFamiliarity value={affect.familiarity} t={t} />

          {(affect.closing || affect.inviteCaution) && (
            <div class="status-affect-badges">
              {affect.closing && (
                <span class="badge status-affect-badge">
                  <Clock size={12} aria-hidden="true" />
                  {t("brain.badge.closing")}
                </span>
              )}
              {affect.inviteCaution && (
                <span class="badge status-affect-badge status-affect-badge--caution">
                  <ShieldAlert size={12} aria-hidden="true" />
                  {t("brain.badge.inviteCaution")}
                </span>
              )}
            </div>
          )}

          <div class="status-affect-drives">
            <h3 class="status-affect-drives-title">{t("status.affect.drives.title")}</h3>
            {top.length === 0 ? (
              <p class="status-affect-empty">{t("status.affect.drives.baseline")}</p>
            ) : (
              <ul class="status-affect-drive-list">
                {top.map((d) => (
                  <MiniDrive
                    key={d.key}
                    driveKey={d.key}
                    level={d.level}
                    base={d.base}
                    delta={d.delta}
                    t={t}
                  />
                ))}
              </ul>
            )}
          </div>

          <p class="status-affect-hint">{t("status.affect.hint")}</p>
        </>
      )}
    </section>
  );
}

export function StatusPanel({ connectionState, version, character, modules, affect }: StatusPanelProps) {
  const { t } = useI18n();
  const moduleEntries = Object.entries(modules).sort(([a], [b]) => a.localeCompare(b));

  return (
    <div class="status-panel">
      <AffectCard affect={affect} t={t} />

      <section class="status-card">
        <div class="status-card-title">
          <Activity size={16} />
          <span>{t("status.title")}</span>
        </div>

        <dl class="status-rows">
          <div class="status-row">
            <span class="status-row-label">{t("status.connection")}</span>
            <span class="status-row-value">
              <ConnectionStatus state={connectionState} />
            </span>
          </div>
          <div class="status-row">
            <span class="status-row-label">{t("status.version")}</span>
            <span class="status-row-value">{version ?? "—"}</span>
          </div>
          <div class="status-row">
            <span class="status-row-label">{t("status.character")}</span>
            <span class="status-row-value">{character?.name ?? t("status.character.none")}</span>
          </div>
        </dl>

        <div class="status-modules">
          <h3 class="status-modules-title">{t("status.modules.title")}</h3>
          {moduleEntries.length === 0 ? (
            <p class="status-modules-empty">{t("status.modules.empty")}</p>
          ) : (
            <ul class="status-modules-list">
              {moduleEntries.map(([name, on]) => (
                <li key={name} class="status-module-item">
                  <span class={`status-module-dot ${on ? "is-on" : "is-off"}`} />
                  <span class="status-module-name">{name}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </section>
    </div>
  );
}
