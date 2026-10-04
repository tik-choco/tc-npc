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
//
// The returned `atBottom` / `scrollToBottom` pair exists so a view can offer
// the way back: once the follow has been dropped (the user scrolled up),
// nothing else would return them to the live edge except manual scrolling.
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import type { RefObject } from "preact";

/** How close to the bottom edge still counts as "following the live edge".
 *  Exported because anything else that scrolls one of these containers has to
 *  agree on where the follow stops — see ChatView's `followGrowth`, which
 *  keeps the transcript pinned while a bubble animates taller. */
export const NEAR_BOTTOM_PX = 120;

export interface AutoScroll {
  /** Attach to the scrolling container. */
  ref: RefObject<HTMLDivElement>;
  /** Whether the container is currently within `NEAR_BOTTOM_PX` of its
   *  bottom edge — i.e. whether new content is being followed. Starts true,
   *  since an empty/short transcript is trivially at its own bottom. */
  atBottom: boolean;
  scrollToBottom: (behavior?: ScrollBehavior) => void;
}

export function useAutoScroll(dep: unknown): AutoScroll {
  const ref = useRef<HTMLDivElement | null>(null);
  const [atBottom, setAtBottom] = useState(true);
  // Mirrors `atBottom` so the scroll handler can compare without being
  // re-created on every flip: scroll events fire far more often than the
  // value actually changes, and only the changes are worth a re-render.
  const atBottomRef = useRef(true);

  const sync = useCallback(() => {
    const el = ref.current;
    if (!el) return true;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
    if (near !== atBottomRef.current) {
      atBottomRef.current = near;
      setAtBottom(near);
    }
    return near;
  }, []);

  const scrollToBottom = useCallback(
    (behavior: ScrollBehavior = "smooth") => {
      const el = ref.current;
      if (!el) return;
      el.scrollTo({ top: el.scrollHeight, behavior });
      // A smooth scroll lands asynchronously, so `sync()` here would still
      // read the pre-scroll offset. The container's own scroll events drive
      // the flag the rest of the way; this just covers the reduced-motion
      // case where the jump is instant and may not emit one.
      sync();
    },
    [sync],
  );

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onScroll = () => sync();
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [sync]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (atBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    } else {
      // Still worth a re-check: content growing above the viewport shifts
      // the bottom further away, and the container emits no scroll event
      // for that on its own.
      sync();
    }
  }, [dep, sync]);

  return { ref, atBottom, scrollToBottom };
}
