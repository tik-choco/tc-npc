import { LLM_SETTINGS_MESSAGES, type LlmSettingsLocale } from "@tik-choco/mistai/preact";
import type { Lang } from "./i18n";
export const AI_MESSAGES = {
  en: { bridge: "Rooms are routed by the server through mistl. Sharing settings are saved here; mistl owns discovery and providing.", serverDefault: "Follow default model", keyRequired: "Enter the API key again before changing the endpoint host.", taskTalk: "Conversation", taskMemory: "Memory", taskEmbedding: "Embedding", taskVision: "Vision", taskAction: "Action", taskTranslation: "Interpretation", taskTts: "Speech synthesis", taskStt: "Speech recognition" },
  ja: { bridge: "ルームはサーバーから mistl 経由で利用します。提供設定は保存されますが、検出・提供は mistl が管理します。", serverDefault: "既定モデルに従う", keyRequired: "接続先のホストを変更するには API key を入力し直してください。", taskTalk: "対話", taskMemory: "記憶", taskEmbedding: "埋め込み", taskVision: "視覚", taskAction: "行動", taskTranslation: "通訳", taskTts: "音声合成", taskStt: "音声認識" },
  "zh-CN": { bridge: "服务器通过 mistl 使用房间。此处保存共享设置；发现和共享由 mistl 管理。", serverDefault: "使用默认模型", keyRequired: "更改连接主机前，请重新输入 API 密钥。", taskTalk: "对话", taskMemory: "记忆", taskEmbedding: "嵌入", taskVision: "视觉", taskAction: "行动", taskTranslation: "口译", taskTts: "语音合成", taskStt: "语音识别" },
  "zh-TW": { bridge: "伺服器透過 mistl 使用房間。此處儲存分享設定；探索與分享由 mistl 管理。", serverDefault: "使用預設模型", keyRequired: "變更連線主機前，請重新輸入 API 金鑰。", taskTalk: "對話", taskMemory: "記憶", taskEmbedding: "嵌入", taskVision: "視覺", taskAction: "行動", taskTranslation: "口譯", taskTts: "語音合成", taskStt: "語音辨識" },
};
export function aiLocale(lang: Lang): LlmSettingsLocale { return lang === "zh" ? "zh-CN" : lang; }
export function aiTranslate(lang: Lang) {
  const locale = aiLocale(lang);
  const messages = { ...LLM_SETTINGS_MESSAGES[locale], ...AI_MESSAGES[locale] };
  return (key: keyof typeof messages, params: Record<string, string | number> = {}) =>
    messages[key].replace(/\{(\w+)\}/g, (whole, name: string) => name in params ? String(params[name]) : whole);
}
export type AiTranslate = ReturnType<typeof aiTranslate>;