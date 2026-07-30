// Hash-based routing, so every tab has its own URL and several browser
// windows can sit side by side on different tabs.
//
// Why the hash and not real paths: vite.config.ts builds with `base: "./"`,
// so index.html references its assets relatively. The Rust binary's SPA
// fallback (crates/npc-server/src/assets.rs) does serve index.html for any
// unmatched path, but a two-segment deep link like `/chat/status` would then
// resolve `./assets/…` against `/chat/`, miss, and fall back to index.html
// again — the bundle would never load. A fragment sidesteps that entirely:
// the document URL stays `/`, so it works at any nesting depth, under any
// future subpath mount, and without touching the server or the build.
//
// Route shape: `#/<tab>` for every tab, plus `#/chat/<panel>` for the チャット
// tab's sidebar pill (音声 / 状態 / 通訳), which is itself a tab the operator
// may want split across windows, and an optional `/avatar` after that for the
// チャット tab's avatar-led layout (`#/chat/status/avatar`).
//
// `#/avatar` is a route but not a header tab: it is the bare avatar window,
// opened from the チャット toolbar into a window of its own (see AvatarView)
// for a second monitor or a capture source, so it deliberately doesn't take
// a slot in the nav.
import { useCallback, useEffect, useState } from "preact/hooks";

export const TAB_IDS = [
  "chat",
  "characters",
  "people",
  "vision",
  "action",
  "schedule",
  "brain",
  "settings",
  "avatar",
] as const;
export type Tab = (typeof TAB_IDS)[number];
export const DEFAULT_TAB: Tab = "chat";

export const CHAT_PANEL_IDS = ["voice", "status", "interpret"] as const;
export type ChatPanel = (typeof CHAT_PANEL_IDS)[number];
export const DEFAULT_CHAT_PANEL: ChatPanel = "voice";

/** How the チャット tab arranges itself. `chat` is the transcript-led layout
 *  the tab has always had; `avatar` puts the VRM in front and lays the
 *  conversation over it. */
export const CHAT_LAYOUT_IDS = ["chat", "avatar"] as const;
export type ChatLayout = (typeof CHAT_LAYOUT_IDS)[number];
export const DEFAULT_CHAT_LAYOUT: ChatLayout = "chat";

const CHAT_PANEL_STORAGE_KEY = "tc-npc:chat-sidebar-tab";
const CHAT_LAYOUT_STORAGE_KEY = "tc-npc:chat-layout";

export interface Route {
  tab: Tab;
  chatPanel: ChatPanel;
  chatLayout: ChatLayout;
}

function isTab(value: string): value is Tab {
  return (TAB_IDS as readonly string[]).includes(value);
}

function isChatPanel(value: string): value is ChatPanel {
  return (CHAT_PANEL_IDS as readonly string[]).includes(value);
}

function isChatLayout(value: string): value is ChatLayout {
  return (CHAT_LAYOUT_IDS as readonly string[]).includes(value);
}

/** Last sidebar panel this browser used, so a window opened at a bare
 *  `#/chat` lands where the operator left off. Defensive in the same way as
 *  lib/i18n.ts's detectLang(): storage can throw in private mode, and a
 *  stale or hand-edited value might not be a known panel id. */
function loadChatPanel(): ChatPanel {
  try {
    const raw = localStorage.getItem(CHAT_PANEL_STORAGE_KEY);
    if (raw !== null && isChatPanel(raw)) return raw;
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return DEFAULT_CHAT_PANEL;
}

/** Best-effort: if the write fails the panel choice still works for this
 *  session, it just won't seed the next window opened without one in its URL. */
function saveChatPanel(panel: ChatPanel): void {
  try {
    localStorage.setItem(CHAT_PANEL_STORAGE_KEY, panel);
  } catch {
    // Non-fatal — the choice just won't be remembered next visit.
  }
}

/** Same shape as `loadChatPanel`, for the transcript/avatar layout choice. */
function loadChatLayout(): ChatLayout {
  try {
    const raw = localStorage.getItem(CHAT_LAYOUT_STORAGE_KEY);
    if (raw !== null && isChatLayout(raw)) return raw;
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return DEFAULT_CHAT_LAYOUT;
}

function saveChatLayout(layout: ChatLayout): void {
  try {
    localStorage.setItem(CHAT_LAYOUT_STORAGE_KEY, layout);
  } catch {
    // Non-fatal — the choice just won't be remembered next visit.
  }
}

/** Unknown/absent segments fall back rather than erroring: a hand-typed or
 *  stale URL should land somewhere sensible, not on a blank screen. */
function parseHash(hash: string): Route {
  const segments = hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  const [rawTab, rawPanel, rawLayout] = segments;
  return {
    tab: rawTab !== undefined && isTab(rawTab) ? rawTab : DEFAULT_TAB,
    chatPanel: rawPanel !== undefined && isChatPanel(rawPanel) ? rawPanel : loadChatPanel(),
    chatLayout: rawLayout !== undefined && isChatLayout(rawLayout) ? rawLayout : loadChatLayout(),
  };
}

/** The sidebar panel and layout only appear in the URL on the チャット tab,
 *  where they are visible — carrying them around on `#/brain` would put
 *  controls the operator can't see into a URL they might copy. The layout
 *  segment is further omitted at its default, so the everyday URL stays the
 *  `#/chat/<panel>` it has always been. */
export function formatHash(route: Route): string {
  if (route.tab !== "chat") return `#/${route.tab}`;
  const base = `#/chat/${route.chatPanel}`;
  return route.chatLayout === DEFAULT_CHAT_LAYOUT ? base : `${base}/${route.chatLayout}`;
}

export interface UseRouteResult {
  route: Route;
  /** Tab navigation goes through the header's `<a href>`s, so the only route
   *  changes that need a programmatic entry point are the sidebar pill and
   *  the layout toggle. */
  setChatPanel: (panel: ChatPanel) => void;
  setChatLayout: (layout: ChatLayout) => void;
}

export function useRoute(): UseRouteResult {
  const [route, setRoute] = useState<Route>(() => parseHash(window.location.hash));

  useEffect(() => {
    // Canonicalize whatever we were opened with (`/`, `#/chat`, a typo) so the
    // address bar always fully describes the view and is safe to copy into
    // another window. replaceState rather than assigning location.hash: this
    // is a correction to the current entry, not a navigation, and it must not
    // leave a bogus entry for Back to land on.
    const canonical = formatHash(parseHash(window.location.hash));
    if (window.location.hash !== canonical) {
      window.history.replaceState(null, "", canonical);
    }

    // Back/forward, and the operator editing the fragment by hand. Both
    // events are listened for because which one fires on traversal between
    // fragment-only entries varies by browser; parseHash is idempotent, so
    // handling both is harmless.
    const sync = () => setRoute(parseHash(window.location.hash));
    window.addEventListener("hashchange", sync);
    window.addEventListener("popstate", sync);
    return () => {
      window.removeEventListener("hashchange", sync);
      window.removeEventListener("popstate", sync);
    };
  }, []);

  // Assigning location.hash (rather than pushState) both pushes the history
  // entry and fires hashchange, so Back works without a second code path.
  // State is set here too so the render doesn't wait on the event.
  const setChatPanel = useCallback((chatPanel: ChatPanel) => {
    saveChatPanel(chatPanel);
    const next: Route = { ...parseHash(window.location.hash), chatPanel };
    setRoute(next);
    const hash = formatHash(next);
    if (window.location.hash !== hash) window.location.hash = hash;
  }, []);

  const setChatLayout = useCallback((chatLayout: ChatLayout) => {
    saveChatLayout(chatLayout);
    const next: Route = { ...parseHash(window.location.hash), chatLayout };
    setRoute(next);
    const hash = formatHash(next);
    if (window.location.hash !== hash) window.location.hash = hash;
  }, []);

  return { route, setChatPanel, setChatLayout };
}
