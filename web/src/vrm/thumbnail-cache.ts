// Persistent cache for キャラ tab model-list thumbnails, keyed by VRM file
// name. Rendering a thumbnail means downloading and parsing the whole model
// (see thumbnail.ts) — these files run 10-50MB, so doing that on every visit
// to the tab would be both slow and a waste of exactly the bandwidth/memory
// this feature exists to avoid. The point of this module is that a given
// model is only ever rendered once, on whichever machine first shows it.
//
// IndexedDB rather than localStorage:
// - localStorage's ~5MB quota is shared with everything else the app keeps
//   there (camera-orbit memory, i18n/layout prefs, ...); a handful of cached
//   thumbnails is nowhere near that today, but there's no reason to spend
//   any of that shared budget on images when IndexedDB has its own, far
//   larger one.
// - localStorage only stores strings, so an image would have to be a base64
//   data URL — roughly 33% bigger than the binary bytes actually are.
//   IndexedDB stores a `Blob` natively, no encoding tax.
// - localStorage's API is synchronous, so a multi-hundred-KB write blocks
//   the main thread. IndexedDB is async by construction.
//
// This module is intentionally dependency-free (no three.js, no app types)
// so CharactersView.tsx can import it statically — only thumbnail.ts itself
// (the part that actually touches three.js) needs the dynamic `import()`.

const DB_NAME = "tc-npc-vrm-thumbnails";
const DB_VERSION = 1;
const STORE_NAME = "thumbnails";

interface ThumbnailRecord {
  file: string;
  /** The server's reported byte size at render time — GET /api/vrm already
   *  hands us this as a cheap "did the underlying file change" signal, so a
   *  model replaced under the same file name (a re-export, a fixed rig,
   *  etc.) doesn't keep showing a stale picture forever. Not a substitute
   *  for a real content hash, but good enough for a cosmetic preview and
   *  free — no extra request or hashing pass required. */
  size: number;
  blob: Blob;
}

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      req.result.createObjectStore(STORE_NAME, { keyPath: "file" });
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/**
 * Look up a cached thumbnail for `file`, valid only if `size` matches what
 * was recorded when it was rendered. Returns an object URL on a hit (the
 * caller owns it and must `URL.revokeObjectURL` it eventually), or null on
 * any miss, size mismatch, or IndexedDB failure — private browsing can
 * refuse to open the database at all, and a thumbnail is a nicety never
 * worth surfacing as an error over.
 */
export async function getCachedThumbnail(file: string, size: number): Promise<string | null> {
  try {
    const db = await openDb();
    const record = await new Promise<ThumbnailRecord | undefined>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const req = tx.objectStore(STORE_NAME).get(file);
      req.onsuccess = () => resolve(req.result as ThumbnailRecord | undefined);
      req.onerror = () => reject(req.error);
    });
    db.close();
    if (!record || record.size !== size) return null;
    return URL.createObjectURL(record.blob);
  } catch {
    return null;
  }
}

/** Store a rendered thumbnail for `file`, tagged with the `size` it was
 *  rendered from. Best-effort: a failed write just means this session's
 *  render gets redone next visit, not a broken feature. */
export async function setCachedThumbnail(file: string, size: number, blob: Blob): Promise<void> {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      const record: ThumbnailRecord = { file, size, blob };
      tx.objectStore(STORE_NAME).put(record);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    db.close();
  } catch {
    // Non-fatal — see getCachedThumbnail.
  }
}

/** Drop a cached thumbnail, e.g. when its model is removed from the library
 *  so the store doesn't grow forever with pictures of deleted models. */
export async function deleteCachedThumbnail(file: string): Promise<void> {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      tx.objectStore(STORE_NAME).delete(file);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    db.close();
  } catch {
    // Non-fatal — see getCachedThumbnail.
  }
}
