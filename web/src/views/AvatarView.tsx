// `#/avatar` — the bare avatar window.
//
// Nothing but the NPC's VRM: no app header, no transcript, no composer. It
// is a route rather than a header tab because its whole point is to be
// opened into a window of its own (from the チャット toolbar's pop-out
// button) and then left alone — on a second monitor, or as a capture source
// for streaming software. Its controls therefore stay hidden until the
// pointer is actually over the window.
//
// It shares the same socket as every other tab, so the model lip-syncs to
// the same `speaking` frames and wears the same affect-derived expression as
// the avatar in the チャット tab; the two stay in step with no extra wiring.
import { useState } from "preact/hooks";
import { Frame, Image as ImageIcon, Loader2, Mic, MicOff, Subtitles, Volume2 } from "lucide-preact";
import { SpriteStage } from "../components/SpriteStage";
import { VrmStage } from "../components/VrmStage";
import type { VrmFraming } from "../vrm/stage";
import type { AffectSnapshot, TtsLineEntry } from "../hooks/useNpcSocket";
import type { AvatarRef, CharacterRef } from "../lib/types";
import type { SpeakingLevelReading } from "../vrm/level";
import { emotionFromAffect } from "../lib/vrm-emotion";
import { useI18n } from "../hooks/useI18n";
import type { MessageKey, Translate } from "../lib/i18n";
import "../styles/components.css";
import "../styles/avatar.css";

/** What sits behind the model. `normal` is the app background; the other two
 *  exist for capture setups — a flat key colour, or nothing at all for a
 *  browser/window arrangement that is itself transparent. */
type Backdrop = "normal" | "chroma" | "transparent";

/** What the NPC is doing right now, at the resolution an operator glancing
 *  at the model actually needs — finer detail (the affect drives, the STT/
 *  TTS internals) already lives in the 状態/感情 tabs. Shared by this window
 *  and the チャット tab's avatar-led layout (`AvatarStatusBadge` below) so
 *  the two read identically instead of drifting into two designs. Modeled
 *  after tc-town's VoiceView `CallStatus`/`STATUS_LABEL` (聞き取り中/考え中/
 *  話し中), but the precedence had to be worked out fresh — see
 *  `deriveAvatarStatus` — because the signals here can overlap in ways a
 *  phone call's can't (the mic stays open through TTS playback so the
 *  operator can barge in, so "listening" and "speaking" aren't actually
 *  mutually exclusive underneath, even though only one is ever shown). */
export type AvatarStatus = "idle" | "listening" | "thinking" | "speaking";

/**
 * Picks one status from the raw socket signals. Precedence, highest first:
 *
 * 1. `speaking` — the NPC's own voice is audible right now. Wins outright
 *    even though `voiceActive` is usually still true underneath (barge-in
 *    keeps the mic open through playback): this indicator exists to explain
 *    the mouth motion that's already on screen, so reporting "listening"
 *    instead would hide the one thing actually happening.
 * 2. `pending` → "thinking" — an input (typed or spoken) was accepted and
 *    no reply has landed yet. Checked independently of `voiceActive`: a
 *    typed message with the voice loop off still spends real time waiting
 *    on the LLM, and deserves the same label a spoken one gets.
 * 3. `voiceActive` → "listening" — only once neither of the above holds,
 *    and only while the loop itself is actually on. A tab with the loop
 *    off is not "listening" just because someone is looking at it.
 * 4. Otherwise "idle" — loop off, nothing in flight, nothing audible.
 */
export function deriveAvatarStatus(state: {
  voiceActive: boolean;
  pending: boolean;
  speaking: boolean;
}): AvatarStatus {
  if (state.speaking) return "speaking";
  if (state.pending) return "thinking";
  if (state.voiceActive) return "listening";
  return "idle";
}

const STATUS_LABEL_KEY: Record<AvatarStatus, MessageKey> = {
  idle: "avatar.status.idle",
  listening: "avatar.status.listening",
  thinking: "avatar.status.thinking",
  speaking: "avatar.status.speaking",
};

const STATUS_ICON: Record<AvatarStatus, typeof Mic> = {
  idle: MicOff,
  listening: Mic,
  thinking: Loader2,
  speaking: Volume2,
};

/**
 * The pill both avatar surfaces render (this window's overlay, and
 * ChatView's avatar-led layout — see avatar.css `.avatar-status`). `volume`
 * only matters while `status === "listening"`: it drives a halo around the
 * glyph the same way the チャット toolbar's own voice toggle does (see
 * `.chat-voice` in styles/chat.css), so the label is backed by visible
 * proof the mic is actually picking something up rather than a bare claim.
 * The rule is duplicated in avatar.css rather than shared with chat.css,
 * which isn't ours to touch this round and already lives in its own
 * stylesheet. Thinking/speaking get a fixed spin/pulse instead of hooking
 * into a live level: `speakingLevelRef` is deliberately a ref rather than
 * state so it can feed the model's mouth at ~20Hz without re-rendering
 * anything else, and reading it here to drive this badge would undo that.
 */
export function AvatarStatusBadge({ status, volume, t }: { status: AvatarStatus; volume: number; t: Translate }) {
  const Icon = STATUS_ICON[status];
  const level = Math.max(0, Math.min(1, volume));
  return (
    <div
      class={`avatar-status avatar-status--${status}`}
      style={status === "listening" ? `--status-level:${level}` : undefined}
      role="status"
    >
      <span class="avatar-status-glyph">
        <Icon size={13} aria-hidden="true" />
      </span>
      <span class="avatar-status-label">{t(STATUS_LABEL_KEY[status])}</span>
    </div>
  );
}

export interface AvatarViewProps {
  /** Only used for the caption's name — the model comes from `avatar`, so
   *  this window works with no character sheet loaded. */
  character: CharacterRef | null;
  avatar: AvatarRef | null;
  /** Server-reported: the NPC's voice is audible right now. */
  speaking: boolean;
  /** Live loudness of the voice audible right now. A ref rather than a
   *  value so its ~20Hz updates don't re-render this window — see VrmStage. */
  speakingLevelRef: { current: SpeakingLevelReading };
  affect: AffectSnapshot | null;
  /** Latest spoken lines, for the optional caption strip. */
  ttsLines: TtsLineEntry[];
  /** Server-reported position of the voice loop's 開始/停止 switch — see
   *  `deriveAvatarStatus`. This window has no toggle of its own; the loop
   *  is only ever started/stopped from the チャット tab's toolbar. */
  voiceActive: boolean;
  /** Live mic level, 0..1 — shapes the status pill's halo while listening,
   *  the same way `volume` halos the チャット toolbar's own voice toggle. */
  volume: number;
  /** True from input-accepted to reply-landed — see `deriveAvatarStatus`. */
  pending: boolean;
}

// This window is deliberately set up once and left running (a second
// monitor, a capture source), so losing framing/backdrop/captions on every
// reload would undo that setup every single time. Persisted the same
// defensive way as lib/router.ts's chat-panel/layout storage: reads/writes
// can throw (private browsing) and a stored value can be stale or hand-
// edited, so every loader falls back to the original default rather than
// letting a bad value break the window.
const FRAMING_STORAGE_KEY = "tc-npc:avatar-window:framing";
const BACKDROP_STORAGE_KEY = "tc-npc:avatar-window:backdrop";
const CAPTIONS_STORAGE_KEY = "tc-npc:avatar-window:captions";

function isFraming(value: string): value is VrmFraming {
  return value === "bust" || value === "upper" || value === "full";
}

function isBackdrop(value: string): value is Backdrop {
  return value === "normal" || value === "chroma" || value === "transparent";
}

function loadFraming(): VrmFraming {
  try {
    const raw = localStorage.getItem(FRAMING_STORAGE_KEY);
    if (raw !== null && isFraming(raw)) return raw;
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return "full";
}

function saveFraming(framing: VrmFraming): void {
  try {
    localStorage.setItem(FRAMING_STORAGE_KEY, framing);
  } catch {
    // Non-fatal — the choice just won't be remembered next visit.
  }
}

function loadBackdrop(): Backdrop {
  try {
    const raw = localStorage.getItem(BACKDROP_STORAGE_KEY);
    if (raw !== null && isBackdrop(raw)) return raw;
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return "normal";
}

function saveBackdrop(backdrop: Backdrop): void {
  try {
    localStorage.setItem(BACKDROP_STORAGE_KEY, backdrop);
  } catch {
    // Non-fatal — the choice just won't be remembered next visit.
  }
}

function loadCaptions(): boolean {
  try {
    const raw = localStorage.getItem(CAPTIONS_STORAGE_KEY);
    if (raw !== null) return raw === "1";
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return true;
}

function saveCaptions(captions: boolean): void {
  try {
    localStorage.setItem(CAPTIONS_STORAGE_KEY, captions ? "1" : "0");
  } catch {
    // Non-fatal — the choice just won't be remembered next visit.
  }
}

export function AvatarView({
  character,
  avatar,
  speaking,
  speakingLevelRef,
  affect,
  ttsLines,
  voiceActive,
  volume,
  pending,
}: AvatarViewProps) {
  const { t } = useI18n();
  const [backdrop, setBackdrop] = useState<Backdrop>(loadBackdrop);
  const [framing, setFraming] = useState<VrmFraming>(loadFraming);
  const [captions, setCaptions] = useState(loadCaptions);

  // Which display mode this body wants. The server has already dropped a
  // reference whose file isn't in the matching library (see `avatar_ref`),
  // so anything that arrives here is renderable — the only question is by
  // which stage. An unrecognised kind draws nothing rather than guessing.
  const sprite = avatar?.kind === "sprite" ? avatar.file : null;
  const file = avatar?.kind === "vrm" ? avatar.file : null;
  const latestLine = ttsLines.length > 0 ? ttsLines[ttsLines.length - 1].text : "";
  const status = deriveAvatarStatus({ voiceActive, pending, speaking });

  const backdropClass =
    backdrop === "chroma" ? " avatar-window--chroma" : backdrop === "transparent" ? " avatar-window--bare" : "";

  if (!file && !sprite) {
    return (
      <div class="avatar-window">
        <div class="avatar-window-empty">
          <div>
            <div class="empty-state-title">{t("avatar.window.empty.title")}</div>
            <div class="empty-state-description">{t("avatar.window.empty.desc")}</div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div class={`avatar-window${backdropClass}`}>
      <div class="avatar-window-controls">
        <button
          type="button"
          class="btn btn-ghost btn-small"
          title={t("avatar.window.framing")}
          onClick={() =>
            setFraming((f) => {
              const next = f === "full" ? "upper" : f === "upper" ? "bust" : "full";
              saveFraming(next);
              return next;
            })
          }
        >
          <Frame size={14} />
          {t(
            framing === "full"
              ? "avatar.window.framing.full"
              : framing === "upper"
                ? "avatar.window.framing.upper"
                : "avatar.window.framing.bust",
          )}
        </button>
        <button
          type="button"
          class="btn btn-ghost btn-small"
          title={t("avatar.window.background")}
          onClick={() =>
            setBackdrop((b) => {
              const next = b === "normal" ? "chroma" : b === "chroma" ? "transparent" : "normal";
              saveBackdrop(next);
              return next;
            })
          }
        >
          <ImageIcon size={14} />
          {t(
            backdrop === "normal"
              ? "avatar.window.background.normal"
              : backdrop === "chroma"
                ? "avatar.window.background.chroma"
                : "avatar.window.background.transparent",
          )}
        </button>
        <button
          type="button"
          class="btn btn-ghost btn-small"
          aria-pressed={captions}
          title={t("avatar.window.caption")}
          onClick={() =>
            setCaptions((v) => {
              const next = !v;
              saveCaptions(next);
              return next;
            })
          }
        >
          <Subtitles size={14} />
        </button>
      </div>

      {/* Gated on the same `captions` toggle as the caption strip below,
          rather than getting a control of its own: this window's whole
          reason to exist is being a clean capture source, so anything drawn
          on top of the model has to be possible to switch off in one place
          — a status pill that couldn't be silenced would be exactly the
          kind of on-screen clutter a chroma-key or transparent capture
          setup is trying to avoid. Not hover-gated like `.avatar-window-
          controls` above: the point of a glanceable status is that it
          doesn't need the pointer parked over the window to be seen. */}
      {captions && (
        <div class="avatar-window-status">
          <AvatarStatusBadge status={status} volume={volume} t={t} />
        </div>
      )}

      {sprite ? (
        // The 2D mode takes the same speech inputs as the 3D one, so the
        // window's framing/backdrop/caption chrome around it is unchanged —
        // only what draws the body differs. Camera framing is not passed:
        // there is no camera.
        <SpriteStage
          file={sprite}
          speaking={speaking}
          speakingLevelRef={speakingLevelRef}
          emotion={emotionFromAffect(affect)}
          label={character?.name}
        />
      ) : (
        <VrmStage
          class="avatar-window-stage"
          file={file!}
          framing={framing}
          speaking={speaking}
          speakingLevelRef={speakingLevelRef}
          emotion={emotionFromAffect(affect)}
          interactive
          initial={[...(character?.name ?? "N").trim()][0] ?? "N"}
        />
      )}

      {captions && (
        <div class="avatar-window-caption">
          {character && <span class="avatar-window-name">{character.name}</span>}
          <span class="avatar-window-line">{latestLine}</span>
        </div>
      )}
    </div>
  );
}
