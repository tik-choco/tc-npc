// Scrolls a transcript container to its newest content, but only when the
// user is already near the bottom — keeps live views (チャット/通訳) pinned
// to new messages without yanking someone back down while they're reading
// older history.
//
// `dep` must change on every new item worth scrolling for (e.g. the id of
// the last entry, or a composite key). Do NOT pass a list's `.length`: these
// transcripts are capped (see MAX_ENTRIES in useNpcSocket.ts), so once a list
// hits its cap its length stops changing and the effect would stop firing
// even though new items keep arriving.
import { useEffect, useRef } from "preact/hooks";

const NEAR_BOTTOM_PX = 120;

export function useAutoScroll(dep: unknown) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
    if (nearBottom) el.scrollTop = el.scrollHeight;
  }, [dep]);
  return ref;
}
