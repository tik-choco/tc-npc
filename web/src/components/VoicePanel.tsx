// チャットサイドバーの「音声」パネル: live volume meter, suspend/resume
// controls, and a log of ttsLine frames (spoken lines + any translations).
//
// The suspend/resume pair here is TTS-only (npc-speech drops queued
// chat_response playback and auto-resumes after a timeout). The *master*
// switch for the whole cascade loop — mic included — is the 音声開始/停止
// toggle in the チャット header; `voiceActive` is passed down only so the
// meter can say why it's reading zero while that switch is off, rather than
// looking like a dead mic.
//
// The device pickers below list the *server's* audio endpoints, not the
// browser's: the mic and speakers belong to the machine running npc-speech,
// and this page is only its remote control. Like InterpretPanel, this panel
// does not own a useConfigDoc — ChatSidebar holds the sidebar's single
// editor and hands it down (see useConfigDoc's header for why there can only
// be one per mounted tree).
import { useEffect, useState } from "preact/hooks";
import { Pause, Play, Volume2, Languages, Mic, Speaker, RefreshCw, AlertTriangle } from "lucide-preact";
import type { TtsLineEntry } from "../hooks/useNpcSocket";
import type { ConfigDocHandle } from "../hooks/useConfigDoc";
import type { SpeechEndpointSection, SpeechSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import type { MessageKey, Translate } from "../lib/i18n";
import { getAudioDevices, type AudioDevices } from "../lib/api";
import { useI18n } from "../hooks/useI18n";
import { SaveChip } from "./SaveChip";
import "../styles/components.css";
import "../styles/voice.css";

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(
    d.getSeconds(),
  ).padStart(2, "0")}`;
}

/** Mirrors capture.rs's VOLUME_UI_GAIN. The `volume` frames on the wire are
 *  raw mic RMS multiplied by this and clamped to [0, 1] (raw speech RMS is
 *  far below 1.0, so an unscaled meter would sit near zero), while
 *  config.stt.input_threshold is compared against the *raw* RMS. Applying
 *  the same factor here is what puts the threshold marker on the bar at the
 *  point the VAD actually switches. Keep in sync with the Rust constant. */
const VOLUME_UI_GAIN = 4;

/** Slider ceiling for the threshold, copied from agent-speech's ControlPanel
 *  (its range is 0–0.2 at 0.001 steps). 0.2 raw RMS lands at 80% of the
 *  meter, so the marker stays on the bar across the whole range. */
const THRESHOLD_MAX = 0.2;
const THRESHOLD_STEP = 0.001;
/** Silence delay bounds. Below ~0.2s the VAD would cut people off mid-
 *  sentence; above ~5s STT latency dominates the conversation. */
const SILENCE_MIN = 0.2;
const SILENCE_MAX = 5;
const SILENCE_STEP = 0.1;
/** Barge-in multiplier bounds. The server clamps anything below 1 (a factor
 *  under the plain VAD threshold would let the agent's own voice, heard back
 *  through an open mic, interrupt every reply), so the slider starts there.
 *  4× is roughly "only a raised voice right next to the mic gets through". */
const BARGE_IN_MIN = 1;
const BARGE_IN_MAX = 4;
const BARGE_IN_STEP = 0.1;

/** config.speech, or {} when the document has no such section yet. */
function readSpeech(config: ConfigDocument | null): SpeechSection {
  return (config?.speech as SpeechSection | undefined) ?? {};
}

/** config.stt, or {} when absent. Only the two VAD knobs are read here —
 *  endpoint/model live in 設定 › タスク. */
function readStt(config: ConfigDocument | null): SpeechEndpointSection {
  return (config?.stt as SpeechEndpointSection | undefined) ?? {};
}

/** input_threshold / silence_duration sit under SpeechEndpointSection's index
 *  signature rather than a named prop (same as TaskRows.readNumber), so a
 *  hand-edited config can put anything there. */
function readNumber(section: SpeechEndpointSection, field: string, fallback: number): number {
  const value = section[field];
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** config.tts, or {} when absent. Only `enabled` is read here (for the log's
 *  empty state below) -- the connection fields live in 設定 › タスク /
 *  provider cards, same split as `readStt`. */
function readTts(config: ConfigDocument | null): SpeechEndpointSection {
  return (config?.tts as SpeechEndpointSection | undefined) ?? {};
}

/**
 * Which empty-state line belongs under the TTS log when there's nothing in
 * it yet. With `ttsLine` now sent per sentence (see TtsLineMessage), an
 * empty log is the *expected* state whenever TTS is off -- config.tts.enabled
 * follows the same "absent means off" convention as SettingsView's own
 * `tts.enabled ?? false` toggle -- and saying so plainly beats leaving a
 * panel that looks like it stopped working. `config === null` (still
 * loading, or the fetch failed) deliberately falls back to the generic
 * "nothing spoken yet" line rather than claiming TTS is off: that claim
 * would be a guess until the document actually says so.
 *
 * A standalone exported function (rather than inlined at the call site) so
 * this one small piece of decision logic is a plain function vitest can
 * exercise without rendering the component -- see VoicePanel.test.ts.
 */
export function ttsLogEmptyKey(config: ConfigDocument | null): MessageKey {
  if (config === null) return "voice.log.empty";
  return (readTts(config).enabled ?? false) ? "voice.log.empty" : "voice.log.empty.ttsOff";
}

/** One labelled range slider with a live numeric readout.
 *
 *  `onInput` moves the caller's draft only; the config write happens on
 *  `change`, which for a range input is pointer release (Preact's onChange is
 *  the native event, not React's input alias — see InterpretSettings). Writing
 *  per input event would deep-clone and PUT the whole config document dozens
 *  of times per drag, and the server applies the value to the running VAD as
 *  soon as it lands, so one write per drag is both cheaper and enough. */
function SliderField(props: {
  label: string;
  hint: string;
  value: number;
  min: number;
  max: number;
  step: number;
  display: string;
  disabled?: boolean;
  onDraft: (value: number) => void;
  onCommit: (value: number) => void;
}) {
  const { label, hint, value, min, max, step, display, disabled, onDraft, onCommit } = props;
  return (
    <div class="voice-slider">
      <div class="voice-slider-head">
        <span class="voice-slider-label">{label}</span>
        <span class="voice-slider-value">{display}</span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onInput={(e) => onDraft(Number((e.currentTarget as HTMLInputElement).value))}
        onChange={(e) => onCommit(Number((e.currentTarget as HTMLInputElement).value))}
      />
      <span class="field-hint">{hint}</span>
    </div>
  );
}

/** Options for one device picker: the OS-default entry (labelled with what
 *  it currently resolves to, when the server told us), then the enumerated
 *  devices. A saved name that no longer enumerates — device unplugged, or a
 *  partial name hand-written into config.json, which the server's substring
 *  match accepts — is kept as its own option: dropping it would make the
 *  <select> silently fall back to 既定 and the next edit would erase a
 *  setting the user never touched. */
function deviceOptions(
  t: Translate,
  names: string[],
  selected: string,
  osDefault: string | null,
): Array<{ value: string; label: string }> {
  const options = [
    {
      value: "",
      label: osDefault ? t("voice.device.default.named", { name: osDefault }) : t("voice.device.default"),
    },
    ...names.map((name) => ({ value: name, label: name })),
  ];
  if (selected !== "" && !names.includes(selected)) {
    options.push({ value: selected, label: t("voice.device.unavailable", { name: selected }) });
  }
  return options;
}

/** 入力/出力デバイス pickers over config.speech.{input,output}_device.
 *  Live: npc-speech restarts just the affected capture/playback thread when
 *  the name changes, so switching headsets doesn't need an app restart. The
 *  stream is re-opened, so a reply that is mid-playback when the output
 *  device changes is cut. */
function VoiceDeviceCard({ doc }: { doc: ConfigDocHandle }) {
  const { t } = useI18n();
  const [devices, setDevices] = useState<AudioDevices | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function refresh() {
    setLoading(true);
    setListError(null);
    try {
      setDevices(await getAudioDevices());
    } catch (err) {
      setDevices(null);
      setListError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  // Enumerated once on mount rather than on every panel switch: the sidebar
  // keeps all panels mounted (ChatSidebar toggles `hidden`), and devices
  // don't come and go often enough to poll. The refresh button covers the
  // "I just plugged in a headset" case.
  useEffect(() => {
    void refresh();
  }, []);

  const speech = readSpeech(doc.config);
  const input = speech.input_device ?? "";
  const output = speech.output_device ?? "";

  const setDevice = (key: "input_device" | "output_device", value: string) => {
    doc.mutate((draft) => {
      const section = readSpeech(draft as ConfigDocument);
      section[key] = value;
      (draft as ConfigDocument).speech = section;
    });
  };

  const pickers: Array<{
    key: "input_device" | "output_device";
    icon: typeof Mic;
    label: string;
    value: string;
    names: string[];
    osDefault: string | null;
  }> = [
    {
      key: "input_device",
      icon: Mic,
      label: t("voice.device.input"),
      value: input,
      names: devices?.input ?? [],
      osDefault: devices?.default_input ?? null,
    },
    {
      key: "output_device",
      icon: Speaker,
      label: t("voice.device.output"),
      value: output,
      names: devices?.output ?? [],
      osDefault: devices?.default_output ?? null,
    },
  ];

  return (
    <section class="voice-device-card">
      <div class="voice-device-header">
        <span class="voice-device-title">{t("voice.device.title")}</span>
        <span class="voice-device-header-spacer" />
        <button
          type="button"
          class="icon-btn"
          onClick={() => void refresh()}
          disabled={loading}
          title={t("voice.device.refresh")}
          aria-label={t("voice.device.refresh")}
        >
          <RefreshCw size={14} class={loading ? "spin" : ""} />
        </button>
      </div>

      {doc.loadError && (
        <p class="voice-device-error">
          <AlertTriangle size={13} />
          {t("voice.device.loadError")}
        </p>
      )}
      {listError && (
        <p class="voice-device-error">
          <AlertTriangle size={13} />
          {t("voice.device.listError")}
        </p>
      )}

      {doc.config === null ? (
        <span class="field-hint">{t("common.loading")}</span>
      ) : (
        pickers.map((picker) => {
          const Icon = picker.icon;
          return (
            <label key={picker.key} class="field voice-device-field">
              <span>
                <Icon size={13} />
                {picker.label}
              </span>
              <select
                value={picker.value}
                onChange={(e) => setDevice(picker.key, (e.target as HTMLSelectElement).value)}
              >
                {deviceOptions(t, picker.names, picker.value, picker.osDefault).map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {opt.label}
                  </option>
                ))}
              </select>
            </label>
          );
        })
      )}
    </section>
  );
}

export interface VoicePanelProps {
  volume: number;
  ttsLines: TtsLineEntry[];
  /** Position of the チャット header's 音声開始/停止 master switch. */
  voiceActive: boolean;
  /** The sidebar's single config editor, shared with InterpretPanel — used
   *  here for the device pickers. */
  config: ConfigDocHandle;
  onSuspend: () => void;
  onResume: () => void;
  /**
   * Server-confirmed result of the last suspend/resume this client sent (see
   * useNpcSocket's `ttsSuspended`) -- null when unknown (nothing sent or
   * acked yet) or when a caller doesn't forward it. Optional so this panel
   * still renders exactly as before for any caller that hasn't wired it
   * through yet; when present it turns the Pause/Resume buttons from
   * fire-and-forget into something that shows whether the last click
   * actually landed.
   */
  ttsSuspended?: boolean | null;
}

export function VoicePanel({
  volume,
  ttsLines,
  voiceActive,
  config,
  onSuspend,
  onResume,
  ttsSuspended,
}: VoicePanelProps) {
  const { t } = useI18n();
  const pct = Math.max(0, Math.min(1, volume)) * 100;

  const stt = readStt(config.config);
  const savedThreshold = readNumber(stt, "input_threshold", 0.01);
  const savedSilence = readNumber(stt, "silence_duration", 1.5);
  const savedBargeFactor = readNumber(stt, "barge_in_factor", 2.5);
  // Absent means the server default, which is on.
  const bargeIn = stt.barge_in !== false;

  // Drag drafts: the sliders track the pointer at once, while the config
  // document only changes on release (see SliderField). Re-synced whenever
  // the saved value changes — on the initial load, and if 設定 › タスク edits
  // the same two fields from the other tab.
  const [threshold, setThreshold] = useState(savedThreshold);
  const [silence, setSilence] = useState(savedSilence);
  const [bargeFactor, setBargeFactor] = useState(savedBargeFactor);
  useEffect(() => setThreshold(savedThreshold), [savedThreshold]);
  useEffect(() => setSilence(savedSilence), [savedSilence]);
  useEffect(() => setBargeFactor(savedBargeFactor), [savedBargeFactor]);

  const patchStt = (patch: Record<string, number | boolean>) => {
    config.mutate((draft) => {
      const section = readStt(draft as ConfigDocument);
      (draft as ConfigDocument).stt = { ...section, ...patch };
    });
  };

  // The threshold is raw RMS, the meter is gain-scaled — convert before
  // placing the marker, and clamp so a hand-edited value past the slider's
  // ceiling parks it at the end of the bar instead of off-screen.
  const thresholdPct = Math.max(0, Math.min(1, threshold * VOLUME_UI_GAIN)) * 100;
  const bargePct = Math.max(0, Math.min(1, threshold * bargeFactor * VOLUME_UI_GAIN)) * 100;
  const overThreshold = voiceActive && volume >= threshold * VOLUME_UI_GAIN;

  return (
    <div class="voice-panel">
      {/* One save indicator for the whole panel: the sliders and the device
          pickers write to the same document, so a per-card chip would light
          up in two places for one edit. Same placement as InterpretPanel. */}
      <div class="voice-panel-head">
        <SaveChip state={config.saveState} error={config.saveError} />
      </div>

      <section class="voice-meter-card">
        <div class="voice-meter-header">
          <Volume2 size={16} />
          <span>{t("voice.micVolume")}</span>
          <span class="voice-meter-value">
            {voiceActive ? `${pct.toFixed(0)}%` : t("chat.voice.off")}
          </span>
        </div>
        <div class="voice-meter-bar">
          <div
            class={`voice-meter-bar-fill${overThreshold ? " voice-meter-bar-fill--active" : ""}`}
            style={{ width: `${pct}%` }}
          />
          {/* Where the VAD's line sits on this bar: fill past the marker is
              what counts as speech. */}
          <div
            class="voice-meter-threshold"
            style={{ left: `${thresholdPct}%` }}
            title={t("voice.tune.threshold")}
          />
          {/* And where the higher barge-in line sits, so you can see how loud
              you have to be to cut a reply off — the number alone doesn't
              tell you that. */}
          {bargeIn && (
            <div
              class="voice-meter-threshold voice-meter-threshold--barge"
              style={{ left: `${bargePct}%` }}
              title={t("voice.bargeIn.factor")}
            />
          )}
        </div>

        <SliderField
          label={t("voice.tune.threshold")}
          hint={t("voice.tune.threshold.hint")}
          value={threshold}
          min={0}
          max={THRESHOLD_MAX}
          step={THRESHOLD_STEP}
          display={threshold.toFixed(3)}
          disabled={config.config === null}
          onDraft={setThreshold}
          onCommit={(value) => patchStt({ input_threshold: value })}
        />

        <SliderField
          label={t("voice.tune.silence")}
          hint={t("voice.tune.silence.hint")}
          value={silence}
          min={SILENCE_MIN}
          max={SILENCE_MAX}
          step={SILENCE_STEP}
          display={t("voice.tune.seconds", { value: silence.toFixed(1) })}
          disabled={config.config === null}
          onDraft={setSilence}
          onCommit={(value) => patchStt({ silence_duration: value })}
        />

        {/* Barge-in lives next to the threshold slider on purpose: its own
            trigger level is a multiple of that value, so the two are only
            tunable as a pair. */}
        <label class="voice-toggle">
          <input
            type="checkbox"
            checked={bargeIn}
            disabled={config.config === null}
            onChange={(e) => patchStt({ barge_in: (e.currentTarget as HTMLInputElement).checked })}
          />
          <span class="voice-toggle-label">{t("voice.bargeIn")}</span>
        </label>
        <span class="field-hint">{t("voice.bargeIn.hint")}</span>

        {bargeIn && (
          <SliderField
            label={t("voice.bargeIn.factor")}
            hint={t("voice.bargeIn.factor.hint")}
            value={bargeFactor}
            min={BARGE_IN_MIN}
            max={BARGE_IN_MAX}
            step={BARGE_IN_STEP}
            display={t("voice.bargeIn.factor.value", { value: bargeFactor.toFixed(1) })}
            disabled={config.config === null}
            onDraft={setBargeFactor}
            onCommit={(value) => patchStt({ barge_in_factor: value })}
          />
        )}

        <span class="field-hint">{t("voice.tune.applies")}</span>

        <div class="voice-controls">
          <button type="button" class="btn btn-ghost" onClick={onResume}>
            <Play size={14} />
            {t("voice.resume")}
          </button>
          <button type="button" class="btn btn-ghost" onClick={onSuspend}>
            <Pause size={14} />
            {t("voice.pause")}
          </button>
        </div>
        {/* Only rendered once a caller actually forwards ttsSuspended and an
            ack has landed (see the prop's doc comment) -- null covers both
            "not wired up" and "nothing acked yet" on purpose, since neither
            has anything honest to display. */}
        {ttsSuspended != null && (
          <span class="field-hint" title={t("voice.suspend.status.tooltip")}>
            {ttsSuspended ? t("voice.suspend.status.suspended") : t("voice.suspend.status.resumed")}
          </span>
        )}
      </section>

      <VoiceDeviceCard doc={config} />

      <section class="voice-log-section">
        <h2 class="voice-log-title">{t("voice.log.title")}</h2>
        {ttsLines.length === 0 && (
          <div class="empty-state">
            <div class="empty-state-title">{t(ttsLogEmptyKey(config.config))}</div>
          </div>
        )}
        <ul class="voice-log-list">
          {ttsLines
            .slice()
            .reverse()
            .map((line) => (
              <li key={line.id} class="voice-log-item">
                <div class="voice-log-item-header">
                  <span class="voice-log-item-text">{line.text}</span>
                  <time>{formatTime(line.ts)}</time>
                </div>
                {line.translations && Object.keys(line.translations).length > 0 && (
                  <ul class="voice-log-translations">
                    {Object.entries(line.translations).map(([lang, text]) => (
                      <li key={lang}>
                        <Languages size={12} />
                        <span class="voice-log-translation-lang">{lang}</span>
                        <span>{text}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
        </ul>
      </section>
    </div>
  );
}
