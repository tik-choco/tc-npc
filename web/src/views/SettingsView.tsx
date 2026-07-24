// 設定 tab: raw JSON editor over GET/PUT /api/config. v1 keeps this
// intentionally simple (a validated textarea, not a generated form) — the
// config schema is owned by the Rust side and can grow without this view
// needing to change. Masked "***" API keys are preserved server-side: as
// long as the operator doesn't touch those fields, PUTting the pretty-
// printed text back round-trips them untouched.
import { useEffect, useState } from "preact/hooks";
import { Save, RotateCcw, Loader2 } from "lucide-preact";
import { getConfig, putConfig } from "../lib/api";
import "../styles/components.css";
import "../styles/settings.css";

export function SettingsView() {
  const [text, setText] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  function load() {
    setLoading(true);
    setError(null);
    setNotice(null);
    getConfig()
      .then((config) => setText(JSON.stringify(config, null, 2)))
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    load();
  }, []);

  async function save() {
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(text);
    } catch (err) {
      setError(`JSON が不正です: ${err instanceof Error ? err.message : String(err)}`);
      return;
    }
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      await putConfig(parsed);
      setNotice("設定を保存しました");
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="settings-view">
      <div class="settings-toolbar">
        <h2 class="settings-title">設定 (JSON)</h2>
        <div class="settings-toolbar-actions">
          <button type="button" class="btn btn-ghost" onClick={load} disabled={loading || saving}>
            <RotateCcw size={14} />
            再読み込み
          </button>
          <button type="button" class="btn btn-primary" onClick={save} disabled={loading || saving}>
            {saving ? <Loader2 size={14} class="spin" /> : <Save size={14} />}
            保存
          </button>
        </div>
      </div>

      <p class="settings-hint">
        "***" と表示されている API キーはそのまま保存すればサーバー側の値が維持されます。書き換えると新しい値で上書きされます。
      </p>

      {error && <div class="settings-error">{error}</div>}
      {notice && <div class="settings-notice">{notice}</div>}

      {loading ? (
        <div class="empty-state">読み込み中…</div>
      ) : (
        <textarea
          class="settings-editor"
          spellcheck={false}
          value={text}
          onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
        />
      )}
    </div>
  );
}
