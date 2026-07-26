import type { ClientMessage, ServerMessage } from "./types";

export type ConnectionState = "connecting" | "open" | "closed";

const RECONNECT_MIN_MS = 1000;
// Deliberately short: the server is a localhost binary that gets restarted a
// lot during development (`just watch`), and waiting out a 30s backoff after
// every rebuild is worse than a few extra failed connects.
const RECONNECT_MAX_MS = 5000;

function wsUrl(): string {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${location.host}/ws`;
}

export interface NpcSocketOptions {
  onMessage: (msg: ServerMessage) => void;
  onStateChange: (state: ConnectionState) => void;
}

/** Reconnecting WebSocket client for the tc-npc /ws endpoint. Backs off
 *  exponentially from 1s to 5s between reconnect attempts — and retries at
 *  once when the tab regains focus/visibility or the network comes back — so
 *  a server restart heals without a page reload. Reports connection-state
 *  transitions via onStateChange so the UI can render a status chip. */
export class NpcSocket {
  private ws: WebSocket | null = null;
  private backoff = RECONNECT_MIN_MS;
  private reconnectTimer: number | undefined;
  private closed = true;
  private readonly opts: NpcSocketOptions;

  constructor(opts: NpcSocketOptions) {
    this.opts = opts;
  }

  connect(): void {
    this.closed = false;
    window.addEventListener("focus", this.retryNow);
    window.addEventListener("online", this.retryNow);
    document.addEventListener("visibilitychange", this.onVisibilityChange);
    this.open();
  }

  /** Skip whatever backoff is pending and reconnect right away. Bound so it
   *  can be used directly as an event listener. Cheap to call when already
   *  connected or connecting — it's a no-op then. */
  private retryNow = (): void => {
    if (this.closed || this.ws) return;
    if (this.reconnectTimer !== undefined) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = undefined;
    }
    this.backoff = RECONNECT_MIN_MS;
    this.open();
  };

  private onVisibilityChange = (): void => {
    if (document.visibilityState === "visible") this.retryNow();
  };

  private open(): void {
    this.opts.onStateChange("connecting");
    const ws = new WebSocket(wsUrl());
    this.ws = ws;

    ws.onopen = () => {
      this.backoff = RECONNECT_MIN_MS;
      this.opts.onStateChange("open");
    };

    ws.onclose = () => {
      this.ws = null;
      this.opts.onStateChange("closed");
      if (this.closed) return;
      const delay = this.backoff;
      this.backoff = Math.min(delay * 2, RECONNECT_MAX_MS);
      this.reconnectTimer = window.setTimeout(() => this.open(), delay);
    };

    ws.onerror = () => {
      ws.close();
    };

    ws.onmessage = (ev: MessageEvent) => {
      try {
        const msg = JSON.parse(ev.data as string) as ServerMessage;
        this.opts.onMessage(msg);
      } catch {
        // Ignore malformed frames.
      }
    };
  }

  send(msg: ClientMessage): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  close(): void {
    this.closed = true;
    window.removeEventListener("focus", this.retryNow);
    window.removeEventListener("online", this.retryNow);
    document.removeEventListener("visibilitychange", this.onVisibilityChange);
    if (this.reconnectTimer !== undefined) window.clearTimeout(this.reconnectTimer);
    this.ws?.close();
    this.ws = null;
  }
}
