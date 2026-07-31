// キャラ tab: character list (GET /api/characters), activate, import a
// tc-town export .json via file picker or drag-drop (POST
// /api/characters/import), and the VRM model library.
//
// The model library is the server's own folder (`{data_dir}/vrm/`), not a
// browser store: tc-npc runs on the operator's machine, so a `.vrm` copied
// into that folder by hand is already in the library and the "add" button
// here is just a way to do that copy without leaving the page. Assigning one
// to a character writes it onto the character file, which is what makes the
// avatar appear in the チャット tab's avatar layout and the `#/avatar`
// window.
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { Boxes, FolderOpen, Image as ImageIcon, Trash2, UploadCloud, UserCheck, Users, Loader2 } from "lucide-preact";
import {
  activateCharacter,
  addSpriteSheet,
  addVrmModel,
  deleteSpriteSheet,
  deleteVrmModel,
  getCharacters,
  getSpriteSheets,
  getVrmModels,
  importCharacter,
  setCharacterAvatar,
  setDefaultAvatar,
  spriteSheetUrl,
  type SpriteSheet,
} from "../lib/api";
import type { CharacterSummary, VrmModel } from "../lib/types";
import { Toast, type ToastState } from "../components/Toast";
import { useI18n } from "../hooks/useI18n";
import type { Translate } from "../lib/i18n";
// Dependency-free (no three.js) — safe to import statically, unlike
// vrm/thumbnail.ts itself, which is reached only through a dynamic
// import() below (see that effect for why).
import { deleteCachedThumbnail, getCachedThumbnail, setCachedThumbnail } from "../vrm/thumbnail-cache";
import "../styles/components.css";
import "../styles/characters.css";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[index]}`;
}

/**
 * The sprite-sheet library: everything in the server's sheet folder, and
 * which sheet the selected character uses.
 *
 * Deliberately much thinner than [`VrmLibrary`] below, for one reason: a
 * sheet's preview *is* the file. VrmLibrary has to spin up three.js to
 * render a model into a picture and cache the result; here the picture is
 * already a picture, so the thumbnail is CSS (`background-size: 800%` shows
 * exactly the first cell of a sheet of `SHEET_COLUMNS` columns, whatever the
 * sheet's pixel size or row count).
 *
 * **Unlike VrmLibrary there is no standalone-default path.** The standalone
 * avatar is `config.character.avatar_file`, which predates there being a
 * second kind and is resolved as a VRM (see npc-server's `avatar_ref`), so a
 * sheet can only be assigned to a character. With no character selected the
 * list is still shown — sheets arrive on import and are worth seeing — but
 * the assign buttons are replaced by the reason they're unavailable, rather
 * than silently doing nothing.
 */
function SpriteLibrary({
  target,
  assignedFile,
  onAssigned,
  onToast,
  t,
}: {
  target: CharacterSummary | null;
  /** The sheet `target` currently uses, so the list can mark it. */
  assignedFile: string | null;
  onAssigned: () => void;
  onToast: (toast: ToastState) => void;
  t: Translate;
}) {
  const [sheets, setSheets] = useState<SpriteSheet[] | null>(null);
  const [dir, setDir] = useState<string>("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragOver, setDragOver] = useState(false);

  function reload() {
    getSpriteSheets()
      .then((res) => {
        setSheets(res.sheets);
        setDir(res.dir);
        setLoadError(null);
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }

  useEffect(reload, []);

  async function handleUpload(file: File) {
    if (!file.name.toLowerCase().endsWith(".png")) {
      onToast({ kind: "error", message: t("sprite.notPngFile") });
      return;
    }
    setBusy(true);
    try {
      const sheet = await addSpriteSheet(file);
      reload();
      // Adding a sheet can make a dangling reference resolve, so the
      // character list is refreshed too — same reasoning as VrmLibrary.
      onAssigned();
      onToast({ kind: "success", message: t("sprite.toast.added", { name: sheet.name }) });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function handleAssign(sheet: SpriteSheet) {
    if (!target) return;
    setBusy(true);
    try {
      await setCharacterAvatar(target.id, sheet.file, "sprite");
      reload();
      onAssigned();
      onToast({ kind: "success", message: t("sprite.toast.assigned") });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete(sheet: SpriteSheet) {
    if (!window.confirm(t("sprite.deleteConfirm", { name: sheet.name }))) return;
    setBusy(true);
    try {
      await deleteSpriteSheet(sheet.file);
      reload();
      // A character still pointing at the deleted sheet keeps the reference:
      // putting the file back restores the avatar, and a dangling one falls
      // back to the initial glyph.
      onAssigned();
      onToast({ kind: "success", message: t("sprite.toast.deleted", { name: sheet.name }) });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="characters-list-section">
      <h2 class="characters-list-title">
        <ImageIcon size={16} />
        {t("sprite.title")}
      </h2>
      <p class="vrm-desc">{t("sprite.desc")}</p>
      {dir && (
        <p class="vrm-dir">
          <FolderOpen size={13} />
          <code>{dir}</code>
        </p>
      )}

      <div
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
          if (file) void handleUpload(file);
        }}
      >
        <UploadCloud size={18} />
        <div>
          <div class="import-dropzone-title">{t("sprite.drop.title")}</div>
          <div class="import-dropzone-sub">{t("sprite.drop.sub")}</div>
        </div>
        <label class="button button--ghost">
          {t("sprite.add")}
          <input
            type="file"
            accept="image/png"
            hidden
            disabled={busy}
            onChange={(e) => {
              const input = e.currentTarget as HTMLInputElement;
              const file = input.files?.[0];
              if (file) void handleUpload(file);
              // Clear so re-picking the same file fires onChange again.
              input.value = "";
            }}
          />
        </label>
      </div>

      {!target && <p class="vrm-usage">{t("sprite.needsCharacter")}</p>}
      {target && <p class="vrm-usage">{t("sprite.usedBy", { name: target.name })}</p>}

      {loadError && <p class="form-error">{loadError}</p>}

      {sheets && sheets.length === 0 && <p class="empty-state-description">{t("sprite.empty")}</p>}

      {sheets && sheets.length > 0 && (
        <ul class="vrm-list">
          {sheets.map((sheet) => {
            const inUse = assignedFile === sheet.file;
            return (
              <li key={sheet.file} class={`vrm-item${inUse ? " vrm-item--current" : ""}`}>
                <div
                  class="sprite-thumb"
                  role="img"
                  aria-label={sheet.name}
                  style={{ backgroundImage: `url(${spriteSheetUrl(sheet.file)})` }}
                />
                <div class="vrm-item-main">
                  <div class="vrm-item-name">{sheet.name}</div>
                  <div class="vrm-item-meta">{formatBytes(sheet.size)}</div>
                </div>
                {inUse ? (
                  <span class="vrm-item-badge">
                    <UserCheck size={13} />
                    {t("sprite.assigned")}
                  </span>
                ) : (
                  target && (
                    <button
                      type="button"
                      class="button button--ghost"
                      disabled={busy}
                      onClick={() => void handleAssign(sheet)}
                    >
                      {t("sprite.assign")}
                    </button>
                  )
                )}
                <button
                  type="button"
                  class="button button--icon"
                  disabled={busy}
                  title={t("sprite.deleteConfirm", { name: sheet.name })}
                  onClick={() => void handleDelete(sheet)}
                >
                  <Trash2 size={14} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/**
 * The VRM library: everything in the server's model folder, and which model
 * is currently in use.
 *
 * "In use" means one of two things, and the difference is what the whole
 * section hinges on:
 * - With a character selected in the list above, a model is assigned to that
 *   character (stored on its character file).
 * - With no characters at all, a model becomes the *standalone* avatar
 *   (`config.character.avatar_file`). The NPC holds a conversation without a
 *   character sheet, so an avatar must not require importing a tc-town
 *   export first — that would put a step in front of the one thing this
 *   feature is for.
 * Either way the button is live; only what it writes changes.
 */
function VrmLibrary({
  target,
  assignedFile,
  onAssigned,
  onToast,
  t,
}: {
  target: CharacterSummary | null;
  /** The model `target` currently uses, so the list can mark it. Ignored
   *  when there is no target — the standalone default is used instead. */
  assignedFile: string | null;
  onAssigned: () => void;
  onToast: (toast: ToastState) => void;
  t: Translate;
}) {
  const [models, setModels] = useState<VrmModel[] | null>(null);
  const [dir, setDir] = useState<string>("");
  const [defaultFile, setDefaultFile] = useState<string>("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Rendered bust preview per model file name — an object URL, either handed
  // back by the IndexedDB cache (vrm/thumbnail-cache.ts) or freshly rendered
  // by the queueing effect below. A file with no entry yet just shows the
  // initial-letter placeholder; nothing here blocks the list itself.
  const [thumbnails, setThumbnails] = useState<Record<string, string>>({});
  // Object URLs handed out by getCachedThumbnail/URL.createObjectURL are only
  // ever created by this component, so it alone is responsible for revoking
  // them on unmount — otherwise every visit to the tab leaks another blob
  // into the page's lifetime.
  const thumbnailUrlsRef = useRef<Set<string>>(new Set());
  // Which (file, size) pairs have already been resolved (from cache or by a
  // fresh render) during this component's lifetime, so that reload() — which
  // replaces `models` with a new array on every add/assign/delete — doesn't
  // re-run the cache lookup (and mint a duplicate object URL) for a model
  // that hasn't actually changed. Keyed on size too, so a model file that
  // gets replaced under the same name is treated as new work rather than
  // silently kept at its old picture.
  const resolvedThumbnailKeysRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    return () => {
      for (const url of thumbnailUrlsRef.current) URL.revokeObjectURL(url);
      thumbnailUrlsRef.current.clear();
    };
  }, []);

  // Populate thumbnails in the background: cache lookups first (cheap, no
  // three.js, safe to run for every model concurrently), then render
  // whichever models missed the cache — one at a time, since rendering means
  // downloading and parsing a full (10-50MB) model and standing up a WebGL
  // context for it. Loading every model at once would mean hundreds of
  // megabytes and as many contexts alive simultaneously; sequential
  // await-in-a-loop keeps exactly one alive at a time, and the list itself
  // is interactive throughout since nothing here blocks render.
  useEffect(() => {
    if (!models) return;
    let cancelled = false;

    void (async () => {
      const misses: VrmModel[] = [];
      for (const model of models) {
        const key = `${model.file} ${model.size}`;
        if (resolvedThumbnailKeysRef.current.has(key)) continue;
        const cached = await getCachedThumbnail(model.file, model.size);
        if (cancelled) return;
        if (cached) {
          resolvedThumbnailKeysRef.current.add(key);
          thumbnailUrlsRef.current.add(cached);
          setThumbnails((prev) => ({ ...prev, [model.file]: cached }));
        } else {
          misses.push(model);
        }
      }
      if (cancelled || misses.length === 0) return;

      // Imported once and reused for the whole queue — see vrm/thumbnail.ts
      // for why this can't be a static import.
      const { renderVrmThumbnail } = await import("../vrm/thumbnail");
      for (const model of misses) {
        if (cancelled) return;
        try {
          const blob = await renderVrmThumbnail(model.file);
          if (cancelled) return;
          const url = URL.createObjectURL(blob);
          resolvedThumbnailKeysRef.current.add(`${model.file} ${model.size}`);
          thumbnailUrlsRef.current.add(url);
          setThumbnails((prev) => ({ ...prev, [model.file]: url }));
          void setCachedThumbnail(model.file, model.size, blob);
        } catch (err) {
          // Left showing the initial-letter fallback — a thumbnail is
          // cosmetic, not worth a toast/error over on a list that otherwise
          // works fine.
          console.error(`failed to render VRM thumbnail for ${model.file}`, err);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [models]);

  // Which model the buttons act on, and which row shows the "in use" badge.
  const activeFile = target ? assignedFile : defaultFile || null;

  const reload = useCallback(() => {
    setLoadError(null);
    getVrmModels()
      .then((res) => {
        setModels(res.models);
        setDir(res.dir);
        setDefaultFile(res.default);
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  async function handleAdd(file: File) {
    if (!file.name.toLowerCase().endsWith(".vrm")) {
      onToast({ kind: "error", message: t("vrm.notVrmFile") });
      return;
    }
    setBusy(true);
    try {
      const model = await addVrmModel(file);
      reload();
      onToast({ kind: "success", message: t("vrm.toast.added", { name: model.name }) });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  /** Assign `file` (or clear, with null) to the character if one is
   *  selected, else to the standalone default. */
  async function applyAvatar(file: string | null, successKey: "vrm.toast.assigned" | "vrm.toast.cleared") {
    setBusy(true);
    try {
      if (target) {
        await setCharacterAvatar(target.id, file);
      } else {
        await setDefaultAvatar(file);
      }
      reload();
      onAssigned();
      onToast({ kind: "success", message: t(successKey) });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete(model: VrmModel) {
    if (!window.confirm(t("vrm.deleteConfirm", { name: model.name }))) return;
    setBusy(true);
    try {
      await deleteVrmModel(model.file);
      // Best-effort tidy-up so the IndexedDB store doesn't grow forever with
      // pictures of models that no longer exist. Not awaited: it has no
      // bearing on whether the delete itself succeeded.
      void deleteCachedThumbnail(model.file);
      reload();
      // A character still pointing at the deleted file keeps the reference:
      // putting the file back restores the avatar, and a dangling one just
      // falls back to the initial glyph. So the list is refreshed too, in
      // case this was the assigned model.
      onAssigned();
      onToast({ kind: "success", message: t("vrm.toast.deleted", { name: model.name }) });
    } catch (err) {
      onToast({ kind: "error", message: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="characters-list-section">
      <h2 class="characters-list-title">
        <Boxes size={16} />
        {t("vrm.title")}
      </h2>
      <p class="vrm-desc">{t("vrm.desc")}</p>
      {dir && (
        <p class="vrm-dir">
          <FolderOpen size={13} />
          <code>{dir}</code>
        </p>
      )}

      <div
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
          if (file) void handleAdd(file);
        }}
      >
        <UploadCloud size={28} />
        <div class="import-dropzone-text">
          <strong>{t("vrm.drop.title")}</strong>
          <span>{t("vrm.drop.sub")}</span>
        </div>
        <button type="button" class="btn btn-ghost btn-small" disabled={busy} onClick={() => inputRef.current?.click()}>
          {busy ? <Loader2 size={14} class="spin" /> : <UploadCloud size={14} />}
          {t("vrm.add")}
        </button>
        <input
          ref={inputRef}
          type="file"
          accept=".vrm,model/gltf-binary"
          class="visually-hidden"
          onChange={(e) => {
            const input = e.target as HTMLInputElement;
            const file = input.files?.[0];
            if (file) void handleAdd(file);
            input.value = "";
          }}
        />
      </div>

      {loadError && (
        <div class="characters-error">
          {t("common.loadFailed")}: {loadError}
        </div>
      )}

      {!models && !loadError && <div class="empty-state">{t("common.loading")}</div>}

      {models && models.length === 0 && <div class="empty-state">{t("vrm.empty")}</div>}

      {models && models.length > 0 && (
        <>
          <p class="vrm-hint">
            {target ? t("vrm.usedBy", { name: target.name }) : t("vrm.usedStandalone")}
          </p>
          <ul class="characters-list">
            {models.map((model) => {
              const inUse = activeFile === model.file;
              const thumbnail = thumbnails[model.file];
              return (
                <li key={model.file} class="character-row">
                  <div class="vrm-thumb">
                    {thumbnail ? (
                      // The name is already the row's own label (see
                      // character-row-name below); the alt text just needs
                      // to say what the picture is of.
                      <img src={thumbnail} alt={model.name} class="vrm-thumb-img" />
                    ) : (
                      // Matches the live avatar stage's own loading/error
                      // placeholder look (.vrm-stage-fallback in
                      // styles/avatar.css) at list-row size; decorative, so
                      // hidden from assistive tech rather than duplicating
                      // the name text right next to it.
                      <span class="vrm-thumb-fallback" aria-hidden="true">
                        {model.name.charAt(0).toUpperCase() || "?"}
                      </span>
                    )}
                  </div>
                  <div class="character-row-main">
                    <span class="character-row-name">{model.name}</span>
                    <span class="character-row-id">{formatBytes(model.size)}</span>
                  </div>
                  {inUse ? (
                    <span class="badge badge--success">
                      <UserCheck size={12} />
                      {t("vrm.assigned")}
                    </span>
                  ) : (
                    <button
                      type="button"
                      class="btn btn-ghost btn-small"
                      disabled={busy}
                      onClick={() => void applyAvatar(model.file, "vrm.toast.assigned")}
                    >
                      <UserCheck size={14} />
                      {target ? t("vrm.assign") : t("vrm.assignStandalone")}
                    </button>
                  )}
                  <button
                    type="button"
                    class="btn btn-ghost btn-small"
                    disabled={busy}
                    title={t("common.delete")}
                    aria-label={t("common.delete")}
                    onClick={() => void handleDelete(model)}
                  >
                    <Trash2 size={14} />
                  </button>
                </li>
              );
            })}
          </ul>
          {activeFile && (
            <button
              type="button"
              class="btn btn-ghost btn-small"
              disabled={busy}
              onClick={() => void applyAvatar(null, "vrm.toast.cleared")}
            >
              {t("vrm.clear")}
            </button>
          )}
        </>
      )}
    </section>
  );
}

export function CharactersView() {
  const { t } = useI18n();
  const [characters, setCharacters] = useState<CharacterSummary[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [toast, setToast] = useState<ToastState | null>(null);
  // Which character the VRM library assigns to. Defaults to whichever is
  // active, since that's the one whose avatar is actually on screen.
  const [selectedId, setSelectedId] = useState<string | null>(null);
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

  // Explicit selection wins; otherwise the active character, which is the
  // one whose avatar is actually being displayed. Resolved against the live
  // list so a selection whose character was removed falls back rather than
  // leaving the library pointing at nothing.
  const target =
    characters?.find((c) => c.id === selectedId) ?? characters?.find((c) => c.active) ?? null;

  async function handleActivate(id: string) {
    setBusyId(id);
    try {
      await activateCharacter(id);
      reload();
      setToast({ kind: "success", message: t("characters.toast.activated") });
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
      setToast({ kind: "success", message: t("characters.toast.imported", { name: file.name }) });
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
          <strong>{t("characters.drop.title")}</strong>
          <span>{t("characters.drop.sub")}</span>
        </div>
        <button
          type="button"
          class="btn btn-ghost btn-small"
          disabled={importing}
          onClick={() => fileInputRef.current?.click()}
        >
          {importing ? <Loader2 size={14} class="spin" /> : <UploadCloud size={14} />}
          {t("characters.pick")}
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
          {t("characters.title")}
        </h2>

        {loadError && (
          <div class="characters-error">
            {t("common.loadFailed")}: {loadError}
          </div>
        )}

        {!characters && !loadError && <div class="empty-state">{t("common.loading")}</div>}

        {characters && characters.length === 0 && (
          <div class="empty-state">
            <div class="empty-state-title">{t("characters.empty.title")}</div>
            <div class="empty-state-description">{t("characters.empty.desc")}</div>
          </div>
        )}

        {characters && characters.length > 0 && (
          <ul class="characters-list">
            {characters.map((c) => (
              <li
                key={c.id}
                class={`character-row character-row--clickable${c.id === target?.id ? " character-row--selected" : ""}`}
                // Selecting a row is what the VRM library below assigns to.
                // The row itself is the target, not a separate dropdown, so
                // "pick a character, then pick its model" reads as one flow.
                onClick={() => setSelectedId(c.id)}
              >
                <div class="character-row-main">
                  <span class="character-row-name">{c.name}</span>
                  <span class="character-row-id">
                    {c.avatar?.kind === "vrm" ? c.avatar.file : t("vrm.notAssigned")}
                  </span>
                </div>
                {c.active ? (
                  <span class="badge badge--success">
                    <UserCheck size={12} />
                    {t("common.enabled")}
                  </span>
                ) : (
                  <button
                    type="button"
                    class="btn btn-ghost btn-small"
                    disabled={busyId === c.id}
                    onClick={(e) => {
                      e.stopPropagation();
                      handleActivate(c.id);
                    }}
                  >
                    {busyId === c.id ? <Loader2 size={14} class="spin" /> : <UserCheck size={14} />}
                    {t("characters.activate")}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <VrmLibrary
        target={target}
        assignedFile={target?.avatar?.kind === "vrm" ? target.avatar.file : null}
        onAssigned={reload}
        onToast={setToast}
        t={t}
      />

      <SpriteLibrary
        target={target}
        assignedFile={target?.avatar?.kind === "sprite" ? target.avatar.file : null}
        onAssigned={reload}
        onToast={setToast}
        t={t}
      />

      {toast && <Toast toast={toast} />}
    </div>
  );
}
