// Shared "edit the server config with silent autosave" hook, used by the
// schedule / action-map / settings views. Edits apply to local state
// immediately and are debounce-flushed to PUT /api/config — no save button,
// matching the tc-* suite's settings convention. Only one view is mounted at
// a time (app.tsx renders tabs conditionally), so a single in-flight editor
// per document is guaranteed; pending edits are flushed on unmount so tab
// switches never lose work.
//
// Masked secrets: GET /api/config returns api_key fields as "***" and the
// server restores them on PUT, so round-tripping the whole document is safe.
// After a save we intentionally do NOT re-fetch — that would clobber a
// freshly typed key back to "***" in the form.
import { useEffect, useRef, useState } from "preact/hooks";

import { getConfig, putConfig } from "../lib/api";
import type { ConfigDocument } from "../lib/types";

export type SaveState = "idle" | "saving" | "saved" | "error";

const SAVE_DEBOUNCE_MS = 600;
const SAVED_INDICATOR_MS = 2500;

export interface ConfigDocHandle {
  /** null until the initial GET resolves (or fails — see loadError). */
  config: ConfigDocument | null;
  loadError: string | null;
  saveState: SaveState;
  saveError: string | null;
  /**
   * Apply an edit: receives a deep-cloned draft of the current document,
   * mutates it in place, and the result becomes the new local state and is
   * debounce-saved. No-op until the initial load has completed.
   */
  mutate: (updater: (draft: ConfigDocument) => void) => void;
  /** Re-fetch from the server, discarding local (already-saved) state. */
  reload: () => Promise<void>;
}

export function useConfigDoc(): ConfigDocHandle {
  const [config, setConfig] = useState<ConfigDocument | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);

  // Latest unsaved document + timer live in refs so the unmount flush sees
  // them without re-registering effects on every keystroke.
  const dirty = useRef<ConfigDocument | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const reload = async () => {
    setLoadError(null);
    try {
      setConfig(await getConfig());
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void reload();
    return () => {
      if (timer.current !== null) clearTimeout(timer.current);
      if (savedTimer.current !== null) clearTimeout(savedTimer.current);
      // Flush pending edits fire-and-forget; the next mount re-fetches.
      if (dirty.current !== null) void putConfig(dirty.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const flush = async () => {
    const doc = dirty.current;
    if (doc === null) return;
    dirty.current = null;
    setSaveState("saving");
    setSaveError(null);
    try {
      await putConfig(doc);
      // Edits made while the PUT was in flight re-arm the timer themselves;
      // only report "saved" if nothing new is pending.
      if (dirty.current === null) {
        setSaveState("saved");
        if (savedTimer.current !== null) clearTimeout(savedTimer.current);
        savedTimer.current = setTimeout(() => setSaveState("idle"), SAVED_INDICATOR_MS);
      }
    } catch (err) {
      setSaveState("error");
      setSaveError(err instanceof Error ? err.message : String(err));
      // Keep the failed document dirty so a later edit (or unmount) retries.
      if (dirty.current === null) dirty.current = doc;
    }
  };

  const mutate = (updater: (draft: ConfigDocument) => void) => {
    setConfig((current) => {
      if (current === null) return current;
      const draft = structuredClone(current);
      updater(draft);
      dirty.current = draft;
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush(), SAVE_DEBOUNCE_MS);
      return draft;
    });
  };

  return { config, loadError, saveState, saveError, mutate, reload };
}
