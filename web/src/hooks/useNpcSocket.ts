// Owns the single NpcSocket connection for the app's lifetime and fans server
// frames out into per-view state slices: a merged `timeline` for the chat
// view (chat + sense + memory + actionLog, in arrival order) plus dedicated
// lists/scalars for the views that only care about one frame kind (音声's
// ttsLines/volume, 視覚's visionLog, 行動's actionLogEntries/position).
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { NpcSocket, type ConnectionState } from "../lib/ws";
import type { ClientMessage, ServerMessage } from "../lib/types";

const MAX_ENTRIES = 500;

export type TimelineEntry =
  | { id: number; kind: "chat"; role: "user" | "assistant"; text: string; ts: number }
  | { id: number; kind: "sense"; senseKind: "vision" | "speech"; text: string; ts: number }
  | { id: number; kind: "memory"; memoryKind: "short" | "long"; text: string; ts: number }
  | { id: number; kind: "actionLog"; text: string; ts: number };

export interface TtsLineEntry {
  id: number;
  text: string;
  translations?: Record<string, string>;
  ts: number;
}

export interface SenseEntry {
  id: number;
  kind: "vision" | "speech";
  text: string;
  ts: number;
}

export interface ActionLogEntry {
  id: number;
  text: string;
  ts: number;
}

export interface ErrorEntry {
  id: number;
  message: string;
  ts: number;
}

export interface PositionState {
  x: number;
  y: number;
  heading: number;
}

export interface UseNpcSocketResult {
  connectionState: ConnectionState;
  version: string | null;
  modules: Record<string, boolean>;
  character: { id: string; name: string } | null;
  timeline: TimelineEntry[];
  ttsLines: TtsLineEntry[];
  visionLog: SenseEntry[];
  actionLogEntries: ActionLogEntry[];
  position: PositionState | null;
  volume: number;
  errors: ErrorEntry[];
  send: (msg: ClientMessage) => void;
}

function cap<T>(list: T[], max: number): T[] {
  return list.length > max ? list.slice(list.length - max) : list;
}

export function useNpcSocket(): UseNpcSocketResult {
  const [connectionState, setConnectionState] = useState<ConnectionState>("connecting");
  const [version, setVersion] = useState<string | null>(null);
  const [modules, setModules] = useState<Record<string, boolean>>({});
  const [character, setCharacter] = useState<{ id: string; name: string } | null>(null);
  const [timeline, setTimeline] = useState<TimelineEntry[]>([]);
  const [ttsLines, setTtsLines] = useState<TtsLineEntry[]>([]);
  const [visionLog, setVisionLog] = useState<SenseEntry[]>([]);
  const [actionLogEntries, setActionLogEntries] = useState<ActionLogEntry[]>([]);
  const [position, setPosition] = useState<PositionState | null>(null);
  const [volume, setVolume] = useState(0);
  const [errors, setErrors] = useState<ErrorEntry[]>([]);

  const socketRef = useRef<NpcSocket | null>(null);
  const seqRef = useRef(0);

  const send = useCallback((msg: ClientMessage) => {
    socketRef.current?.send(msg);
  }, []);

  useEffect(() => {
    function nextId(): number {
      seqRef.current += 1;
      return seqRef.current;
    }

    function handleMessage(msg: ServerMessage) {
      const now = Date.now();
      switch (msg.type) {
        case "hello":
          setVersion(msg.version);
          setModules(msg.modules);
          setCharacter(msg.character);
          break;

        case "status":
          setModules(msg.modules);
          break;

        case "chat":
          setTimeline((prev) =>
            cap([...prev, { id: nextId(), kind: "chat", role: msg.role, text: msg.text, ts: msg.ts }], MAX_ENTRIES),
          );
          break;

        case "sense":
          setTimeline((prev) =>
            cap(
              [...prev, { id: nextId(), kind: "sense", senseKind: msg.kind, text: msg.text, ts: msg.ts }],
              MAX_ENTRIES,
            ),
          );
          if (msg.kind === "vision") {
            setVisionLog((prev) => cap([...prev, { id: nextId(), kind: "vision", text: msg.text, ts: msg.ts }], MAX_ENTRIES));
          }
          break;

        case "memory":
          setTimeline((prev) =>
            cap([...prev, { id: nextId(), kind: "memory", memoryKind: msg.kind, text: msg.text, ts: now }], MAX_ENTRIES),
          );
          break;

        case "actionLog":
          setTimeline((prev) => cap([...prev, { id: nextId(), kind: "actionLog", text: msg.text, ts: now }], MAX_ENTRIES));
          setActionLogEntries((prev) => cap([...prev, { id: nextId(), text: msg.text, ts: now }], MAX_ENTRIES));
          break;

        case "ttsLine":
          setTtsLines((prev) =>
            cap([...prev, { id: nextId(), text: msg.text, translations: msg.translations, ts: now }], MAX_ENTRIES),
          );
          break;

        case "position":
          setPosition({ x: msg.x, y: msg.y, heading: msg.heading });
          break;

        case "volume":
          setVolume(msg.level);
          break;

        case "error":
          setErrors((prev) => cap([...prev, { id: nextId(), message: msg.message, ts: now }], MAX_ENTRIES));
          break;

        case "response":
          if (msg.status === "error") {
            setErrors((prev) => cap([...prev, { id: nextId(), message: msg.message ?? "unknown error", ts: now }], MAX_ENTRIES));
          }
          break;

        case "inputAccepted":
          // Server-side ack of a queued `input`/`command`; no dedicated UI
          // state in this v1 (the resulting `chat`/`actionLog` frame is what
          // actually renders).
          break;

        default:
          break;
      }
    }

    const socket = new NpcSocket({
      onMessage: handleMessage,
      onStateChange: setConnectionState,
    });
    socketRef.current = socket;
    socket.connect();

    return () => {
      socket.close();
      socketRef.current = null;
    };
  }, []);

  return {
    connectionState,
    version,
    modules,
    character,
    timeline,
    ttsLines,
    visionLog,
    actionLogEntries,
    position,
    volume,
    errors,
    send,
  };
}
