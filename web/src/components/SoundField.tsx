// The chime/BGM picker used everywhere `config.scheduler` names an audio
// file: the announcement rows, their `speak` actions, and the scheduler
// defaults.
//
// This replaces what used to be a plain text input holding a path. The path
// hasn't gone away — the server still resolves anything that isn't a bare
// library name (see npc_core::sound::resolve_sound_path) — so the control is
// a select over `{data_dir}/sound/` that *keeps* a value it doesn't
// recognize as its own option rather than silently dropping it. A config
// written before the folder existed therefore survives being rendered, and
// only normalizes to a bare file name if the operator actually picks
// something else.
import type { SoundFile } from "../lib/api";
import type { Translate } from "../lib/i18n";

export interface SoundOption {
  /** What gets stored in `chime_file`/`bgm_file`; `""` means "no sound". */
  value: string;
  label: string;
}

/**
 * The option list for one picker: the "none" entry, every file in the
 * library, and — only when `value` names something not in the library — a
 * trailing entry carrying that value verbatim.
 *
 * That last entry is the whole reason this is a function worth testing. A
 * `<select>` whose current value has no matching option renders as blank and
 * commits that blankness the moment anything else in the row is edited, so a
 * hand-written path like `sound/bgm3.mp3` (or a file that has since been
 * moved out of the folder) would quietly erase itself.
 */
export function soundOptions(value: string, sounds: SoundFile[], noneLabel: string): SoundOption[] {
  const options: SoundOption[] = [{ value: "", label: noneLabel }];
  for (const sound of sounds) {
    options.push({ value: sound.file, label: sound.file });
  }
  if (value !== "" && !sounds.some((sound) => sound.file === value)) {
    options.push({ value, label: value });
  }
  return options;
}

/**
 * A labelled sound picker. Commits on change rather than on blur (unlike the
 * text fields around it): a select has no half-typed intermediate state to
 * protect, so there is nothing to wait for.
 */
export function SoundField({
  t,
  label,
  value,
  sounds,
  onCommit,
  class: className = "schedule-chime-field",
  showLabel = true,
}: {
  t: Translate;
  label: string;
  value: string;
  sounds: SoundFile[];
  onCommit: (value: string) => void;
  class?: string;
  /** Drop the caption and keep it as the select's tooltip. For the action
   *  rows, whose fields sit on one line and have never carried captions. */
  showLabel?: boolean;
}) {
  const options = soundOptions(value, sounds, t("sound.none"));

  return (
    <label class={className} title={showLabel ? undefined : label}>
      {showLabel && <span class="schedule-field-label">{label}</span>}
      <select
        class="sound-select"
        value={value}
        onChange={(e) => onCommit((e.target as HTMLSelectElement).value)}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </label>
  );
}
