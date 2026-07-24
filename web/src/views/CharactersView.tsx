// キャラ tab: character list (GET /api/characters), activate, and import a
// tc-town export .json via file picker or drag-drop (POST
// /api/characters/import).
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { UploadCloud, UserCheck, Users, Loader2 } from "lucide-preact";
import { activateCharacter, getCharacters, importCharacter } from "../lib/api";
import type { CharacterSummary } from "../lib/types";
import { Toast, type ToastState } from "../components/Toast";
import "../styles/components.css";
import "../styles/characters.css";

export function CharactersView() {
  const [characters, setCharacters] = useState<CharacterSummary[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [toast, setToast] = useState<ToastState | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const reload = useCallback(() => {
    setLoadError(null);
    getCharacters()
      .then(setCharacters)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 4000);
    return () => window.clearTimeout(t);
  }, [toast]);

  async function handleActivate(id: string) {
    setBusyId(id);
    try {
      await activateCharacter(id);
      reload();
      setToast({ kind: "success", message: "キャラクターを有効化しました" });
    } catch (err) {
      setToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusyId(null);
    }
  }

  async function handleImportFile(file: File) {
    setImporting(true);
    try {
      const text = await file.text();
      const data = JSON.parse(text);
      await importCharacter(data);
      reload();
      setToast({ kind: "success", message: `「${file.name}」をインポートしました` });
    } catch (err) {
      setToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setImporting(false);
    }
  }

  return (
    <div class="characters-view">
      <section
        class={`import-dropzone${dragOver ? " import-dropzone--active" : ""}`}
        onDragOver={(e) => {
          e.preventDefault();
          setDragOver(true);
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragOver(false);
          const file = e.dataTransfer?.files?.[0];
          if (file) void handleImportFile(file);
        }}
      >
        <UploadCloud size={28} />
        <div class="import-dropzone-text">
          <strong>tc-town エクスポート(.json)をドラッグ&ドロップ</strong>
          <span>またはファイルを選択してインポート</span>
        </div>
        <button
          type="button"
          class="btn btn-ghost btn-small"
          disabled={importing}
          onClick={() => fileInputRef.current?.click()}
        >
          {importing ? <Loader2 size={14} class="spin" /> : <UploadCloud size={14} />}
          ファイルを選択
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept="application/json,.json"
          class="visually-hidden"
          onChange={(e) => {
            const file = (e.target as HTMLInputElement).files?.[0];
            if (file) void handleImportFile(file);
            (e.target as HTMLInputElement).value = "";
          }}
        />
      </section>

      <section class="characters-list-section">
        <h2 class="characters-list-title">
          <Users size={16} />
          キャラクター
        </h2>

        {loadError && <div class="characters-error">読み込みに失敗しました: {loadError}</div>}

        {!characters && !loadError && <div class="empty-state">読み込み中…</div>}

        {characters && characters.length === 0 && (
          <div class="empty-state">
            <div class="empty-state-title">キャラクターがありません</div>
            <div class="empty-state-description">上のインポートから tc-town エクスポートを取り込んでください。</div>
          </div>
        )}

        {characters && characters.length > 0 && (
          <ul class="characters-list">
            {characters.map((c) => (
              <li key={c.id} class="character-row">
                <div class="character-row-main">
                  <span class="character-row-name">{c.name}</span>
                  <span class="character-row-id">{c.id}</span>
                </div>
                {c.active ? (
                  <span class="badge badge--success">
                    <UserCheck size={12} />
                    有効
                  </span>
                ) : (
                  <button
                    type="button"
                    class="btn btn-ghost btn-small"
                    disabled={busyId === c.id}
                    onClick={() => handleActivate(c.id)}
                  >
                    {busyId === c.id ? <Loader2 size={14} class="spin" /> : <UserCheck size={14} />}
                    有効化
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      {toast && <Toast toast={toast} />}
    </div>
  );
}
