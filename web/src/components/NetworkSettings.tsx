// AI Network タブ(設定 > AI Network)。config.mist(crates/npc-core/src/
// config.rs の MistConfig: enabled/signaling_url/room_id)を編集する。
//
// tc-docs/drafts/llm-settings-common-v1.md §3.3 に準拠: 有効/無効トグルの
// カード(.settings-role-card, tc-translate の AI Network コンシューマ/
// プロバイダカードと同型)一枚に集約し、カード直下の説明(.settings-role-desc)
// とフィールドのtooltipで説明を持たせる。ページ冒頭の1文
// (settings.network.tip)だけは常時表示のヒント段落として許容し、それ以外に
// 常時表示の説明文は置かない — 各フィールドの説明は TextField の tooltip
// (title属性のhover)に一本化している。
//
// `MistSection`はサーバ設定のうちこの画面が触れる部分だけを写した最小限の
// 型で、lib/config-types.ts には未定義(編集禁止のため)なのでここにローカル
// 定義する。他のセクション型(TranslationSectionなど)と同じ「フィールドは
// 全部optional、書き込みは既存セクションをspreadしてからpatch」という
// 規約に合わせている。
import { Network } from "lucide-preact";
import { TextField } from "./SettingsFields";
import type { Mutate } from "../lib/llm-config";
import type { Translate } from "../lib/i18n";
import type { ConfigDocument } from "../lib/types";
import "../styles/settings-llm.css";
import "../styles/components.css";

/** config.mist — the AI Network (mist signaling) connection. Mirrors Rust's
 * `MistConfig` (crates/npc-core/src/config.rs). */
interface MistSection {
  enabled?: boolean;
  signaling_url?: string;
  room_id?: string;
}

function readMist(config: ConfigDocument): MistSection {
  return (config.mist as MistSection | undefined) ?? {};
}

export function NetworkSettings(props: { t: Translate; config: ConfigDocument; mutate: Mutate }) {
  const { t, config, mutate } = props;
  const mist = readMist(config);

  function patchMist(patch: Partial<MistSection>): void {
    mutate((draft) => {
      const current = (draft.mist as MistSection | undefined) ?? {};
      draft.mist = { ...current, ...patch };
    });
  }

  return (
    <>
      <p class="field-hint">{t("settings.network.tip")}</p>
      <div class="settings-role-group">
        <div class="settings-role-card">
          <label class="settings-role-head">
            <input
              type="checkbox"
              checked={mist.enabled ?? false}
              onChange={(e) => patchMist({ enabled: (e.target as HTMLInputElement).checked })}
            />
            <span class="settings-role-title">
              <Network size={15} />
              {t("network.enabled")}
            </span>
          </label>
          <p class="settings-role-desc">{t("network.enabled.tooltip")}</p>
          {mist.enabled ? (
            <div class="settings-role-body">
              <TextField
                label={t("network.roomId")}
                tooltip={t("network.roomId.tooltip")}
                value={mist.room_id ?? ""}
                onCommit={(v) => patchMist({ room_id: v })}
              />
              <TextField
                label={t("network.signalingUrl")}
                tooltip={t("network.signalingUrl.tooltip")}
                value={mist.signaling_url ?? ""}
                onCommit={(v) => patchMist({ signaling_url: v })}
              />
            </div>
          ) : null}
        </div>
      </div>
    </>
  );
}
