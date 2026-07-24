// 行動 tab: an interactive map over config.action (locations + routes,
// autosaved via useConfigDoc / hot-reloaded by the server), the live NPC
// position/trail, a command composer, and the action log.
import { useEffect, useRef, useState } from "preact/hooks";
import {
  ChevronDown,
  ChevronUp,
  Compass,
  ListTree,
  MapPinned,
  Navigation,
  Play,
  Plus,
  Route as RouteIcon,
  Send,
  Square,
  Trash2,
} from "lucide-preact";
import type { ActionLogEntry, PositionState } from "../hooks/useNpcSocket";
import type { ActionSection, LocationEntry, RouteEntry } from "../lib/config-types";
import { useConfigDoc, type SaveState } from "../hooks/useConfigDoc";
import { MapCanvas } from "../components/MapCanvas";
import "../styles/components.css";
import "../styles/action.css";

const MAX_TRAIL = 200;

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(
    d.getSeconds(),
  ).padStart(2, "0")}`;
}

function nextName(base: string, existing: Set<string>): string {
  if (!existing.has(base)) return base;
  let i = 2;
  while (existing.has(`${base}${i}`)) i++;
  return `${base}${i}`;
}

function saveStateLabel(state: SaveState): string {
  switch (state) {
    case "saving":
      return "保存中…";
    case "saved":
      return "保存済み";
    case "error":
      return "保存エラー";
    default:
      return "";
  }
}

export interface ActionViewProps {
  position: PositionState | null;
  actionLogEntries: ActionLogEntry[];
  onCommand: (text: string) => void;
}

export function ActionView({ position, actionLogEntries, onCommand }: ActionViewProps) {
  const { config, saveState, mutate } = useConfigDoc();
  const [draft, setDraft] = useState("");
  const [trail, setTrail] = useState<{ x: number; y: number }[]>([]);
  const [addMode, setAddMode] = useState(false);
  const [selectedRouteName, setSelectedRouteName] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [actionLogEntries.length]);

  useEffect(() => {
    if (!position) return;
    setTrail((prev) => {
      const next = [...prev, { x: position.x, y: position.y }];
      return next.length > MAX_TRAIL ? next.slice(next.length - MAX_TRAIL) : next;
    });
  }, [position]);

  const action: ActionSection = (config?.action as ActionSection | undefined) ?? {};
  const locations = action.locations ?? [];
  const routes = action.routes ?? [];
  const selectedRoute = routes.find((r) => r.name === selectedRouteName) ?? null;

  function submit() {
    const text = draft.trim();
    if (!text) return;
    onCommand(text);
    setDraft("");
  }

  // --- Location edits -----------------------------------------------------

  function addLocation(name: string, x: number, y: number) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const locs = a.locations ?? (a.locations = []);
      const used = new Set(locs.map((l) => l.name));
      const finalName = nextName(name.trim() || "地点", used);
      locs.push({ name: finalName, x, y, heading: 0 });
    });
  }

  function moveLocation(name: string, x: number, y: number) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const loc = (a.locations ?? []).find((l) => l.name === name);
      if (loc) {
        loc.x = x;
        loc.y = y;
      }
    });
  }

  function updateLocationField(name: string, field: "name" | "x" | "y" | "heading", raw: string) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const locs = a.locations ?? [];
      const loc = locs.find((l) => l.name === name);
      if (!loc) return;
      if (field === "name") {
        const trimmed = raw.trim();
        if (!trimmed || trimmed === name) return;
        const used = new Set(locs.filter((l) => l !== loc).map((l) => l.name));
        const finalName = nextName(trimmed, used);
        loc.name = finalName;
        for (const r of a.routes ?? []) {
          for (const w of r.waypoints ?? []) {
            if (w.location === name) w.location = finalName;
          }
        }
      } else {
        const n = Number(raw);
        if (!Number.isFinite(n)) return;
        loc[field] = n;
      }
    });
  }

  function removeLocation(name: string) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      a.locations = (a.locations ?? []).filter((l) => l.name !== name);
      for (const r of a.routes ?? []) {
        r.waypoints = (r.waypoints ?? []).filter((w) => w.location !== name);
      }
    });
  }

  // --- Route edits ---------------------------------------------------------

  function addRoute() {
    const used = new Set(routes.map((r) => r.name));
    const name = nextName("ルート", used);
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const list = a.routes ?? (a.routes = []);
      list.push({ name, loop: false, waypoints: [] });
    });
    setSelectedRouteName(name);
  }

  function renameRoute(oldName: string, rawName: string) {
    const trimmed = rawName.trim();
    if (!trimmed || trimmed === oldName) return;
    const used = new Set(routes.filter((r) => r.name !== oldName).map((r) => r.name));
    const finalName = nextName(trimmed, used);
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === oldName);
      if (route) route.name = finalName;
    });
    setSelectedRouteName((cur) => (cur === oldName ? finalName : cur));
  }

  function toggleLoop(name: string, loop: boolean) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === name);
      if (route) route.loop = loop;
    });
  }

  function removeRoute(name: string) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      a.routes = (a.routes ?? []).filter((r) => r.name !== name);
    });
    setSelectedRouteName((cur) => (cur === name ? null : cur));
  }

  function addWaypoint(routeName: string) {
    if (locations.length === 0) return;
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === routeName);
      const firstLoc = (a.locations ?? [])[0];
      if (!route || !firstLoc) return;
      const waypoints = route.waypoints ?? (route.waypoints = []);
      waypoints.push({ location: firstLoc.name, seconds: 3, wait: false });
    });
  }

  function updateWaypoint(routeName: string, index: number, field: "location" | "seconds" | "wait", value: string | boolean) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === routeName);
      const wp = route?.waypoints?.[index];
      if (!wp) return;
      if (field === "location") wp.location = value as string;
      else if (field === "wait") wp.wait = value as boolean;
      else {
        const n = Number(value);
        if (Number.isFinite(n) && n >= 0) wp.seconds = n;
      }
    });
  }

  function moveWaypoint(routeName: string, index: number, dir: -1 | 1) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === routeName);
      const wps = route?.waypoints;
      if (!wps) return;
      const j = index + dir;
      if (j < 0 || j >= wps.length) return;
      const tmp = wps[index];
      wps[index] = wps[j];
      wps[j] = tmp;
    });
  }

  function removeWaypoint(routeName: string, index: number) {
    mutate((doc) => {
      const a: ActionSection = (doc.action as ActionSection | undefined) ?? {};
      doc.action = a;
      const route = (a.routes ?? []).find((r) => r.name === routeName);
      route?.waypoints?.splice(index, 1);
    });
  }

  return (
    <div class="action-view">
      <div class="action-toolbar">
        <h2 class="action-section-title">
          <Compass size={16} />
          行動マップ
        </h2>
        <div class="action-toolbar-right">
          {saveState !== "idle" && <span class={`chip action-save-chip action-save-chip--${saveState}`}>{saveStateLabel(saveState)}</span>}
        </div>
      </div>
      <p class="action-hint">地点・ルートは編集すると自動保存され、すぐに反映されます。</p>

      <section class="action-map-card">
        <MapCanvas
          locations={locations}
          routes={routes}
          selectedRouteName={selectedRouteName}
          position={position}
          trail={trail}
          addMode={addMode}
          onToggleAddMode={() => setAddMode((v) => !v)}
          onAddLocation={addLocation}
          onMoveLocation={moveLocation}
        />
        {!position && <div class="action-position-empty">位置情報は未受信です</div>}
      </section>

      <div class="action-panels">
        <section class="action-panel">
          <h2 class="action-section-title">
            <MapPinned size={16} />
            地点
          </h2>
          {locations.length === 0 ? (
            <div class="empty-state">
              <div class="empty-state-title">地点がありません</div>
              <div class="empty-state-description">地図の「地点追加モード」から作成できます。</div>
            </div>
          ) : (
            <div class="action-loc-list">
              {locations.map((loc) => (
                <LocationRow
                  key={loc.name}
                  loc={loc}
                  onCommit={(field, value) => updateLocationField(loc.name, field, value)}
                  onDelete={() => removeLocation(loc.name)}
                  onGo={() => onCommand(`go ${loc.name}`)}
                />
              ))}
            </div>
          )}
        </section>

        <section class="action-panel">
          <h2 class="action-section-title">
            <RouteIcon size={16} />
            ルート
          </h2>
          <div class="action-route-list">
            {routes.map((route, idx) => (
              <div key={route.name} class={`action-route-item${route.name === selectedRouteName ? " is-selected" : ""}`}>
                <button type="button" class="action-route-select" onClick={() => setSelectedRouteName(route.name)}>
                  <span class="action-route-color" style={{ background: ROUTE_LIST_COLORS[idx % ROUTE_LIST_COLORS.length] }} />
                  {route.name}
                  {route.loop && <span class="chip action-route-loop-chip">ループ</span>}
                </button>
                <button type="button" class="icon-btn" onClick={() => removeRoute(route.name)} title="削除">
                  <Trash2 size={14} />
                </button>
              </div>
            ))}
            <button type="button" class="btn btn-ghost btn-small" onClick={addRoute}>
              <Plus size={14} />
              ルート追加
            </button>
          </div>

          {routes.length === 0 && (
            <div class="empty-state">
              <div class="empty-state-title">ルートがありません</div>
            </div>
          )}

          {selectedRoute && (
            <RouteDetail
              route={selectedRoute}
              locations={locations}
              onRename={(name) => renameRoute(selectedRoute.name, name)}
              onToggleLoop={(loop) => toggleLoop(selectedRoute.name, loop)}
              onRun={() => onCommand(`route ${selectedRoute.name}`)}
              onStop={() => onCommand("stop")}
              onAddWaypoint={() => addWaypoint(selectedRoute.name)}
              onUpdateWaypoint={(i, field, value) => updateWaypoint(selectedRoute.name, i, field, value)}
              onMoveWaypoint={(i, dir) => moveWaypoint(selectedRoute.name, i, dir)}
              onRemoveWaypoint={(i) => removeWaypoint(selectedRoute.name, i)}
            />
          )}
        </section>
      </div>

      <section class="action-command-card">
        <h2 class="action-section-title">
          <Send size={16} />
          コマンド送信
        </h2>
        <div class="action-command-row">
          <input
            type="text"
            placeholder="mv 3 / rt 90 / goto 5 10 / route <name>"
            value={draft}
            onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.isComposing) {
                e.preventDefault();
                submit();
              }
            }}
          />
          <button type="button" class="btn btn-primary" onClick={submit} disabled={!draft.trim()}>
            <Send size={14} />
            送信
          </button>
          <button type="button" class="btn btn-danger" onClick={() => onCommand("stop")}>
            <Square size={14} />
            停止
          </button>
        </div>
      </section>

      <section class="action-log-section">
        <h2 class="action-section-title">
          <ListTree size={16} />
          行動ログ
        </h2>
        <div class="action-log-scroll" ref={scrollRef}>
          {actionLogEntries.length === 0 && (
            <div class="empty-state">
              <div class="empty-state-title">まだ行動ログがありません</div>
            </div>
          )}
          {actionLogEntries.map((entry) => (
            <div key={entry.id} class="action-log-item">
              <time>{formatTime(entry.ts)}</time>
              <span>{entry.text}</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}

const ROUTE_LIST_COLORS = ["#2563eb", "#059669", "#d946ef", "#ea580c", "#0891b2", "#7c3aed"];

function LocationRow({
  loc,
  onCommit,
  onDelete,
  onGo,
}: {
  loc: LocationEntry;
  onCommit: (field: "name" | "x" | "y" | "heading", value: string) => void;
  onDelete: () => void;
  onGo: () => void;
}) {
  const [name, setName] = useState(loc.name);
  const [x, setX] = useState(String(loc.x ?? 0));
  const [y, setY] = useState(String(loc.y ?? 0));
  const [heading, setHeading] = useState(String(loc.heading ?? 0));

  useEffect(() => setName(loc.name), [loc.name]);
  useEffect(() => setX(String(loc.x ?? 0)), [loc.x]);
  useEffect(() => setY(String(loc.y ?? 0)), [loc.y]);
  useEffect(() => setHeading(String(loc.heading ?? 0)), [loc.heading]);

  return (
    <div class="action-loc-row">
      <input
        class="action-loc-name"
        value={name}
        onInput={(e) => setName((e.target as HTMLInputElement).value)}
        onBlur={() => onCommit("name", name)}
      />
      <input
        class="action-loc-num"
        type="number"
        step="0.1"
        value={x}
        onInput={(e) => setX((e.target as HTMLInputElement).value)}
        onBlur={() => onCommit("x", x)}
        title="X"
      />
      <input
        class="action-loc-num"
        type="number"
        step="0.1"
        value={y}
        onInput={(e) => setY((e.target as HTMLInputElement).value)}
        onBlur={() => onCommit("y", y)}
        title="Y"
      />
      <input
        class="action-loc-num"
        type="number"
        step="1"
        value={heading}
        onInput={(e) => setHeading((e.target as HTMLInputElement).value)}
        onBlur={() => onCommit("heading", heading)}
        title="向き"
      />
      <button type="button" class="btn btn-ghost btn-small" onClick={onGo}>
        <Navigation size={13} />
        移動
      </button>
      <button type="button" class="icon-btn" onClick={onDelete} title="削除">
        <Trash2 size={14} />
      </button>
    </div>
  );
}

function RouteNameInput({ name, onCommit }: { name: string; onCommit: (value: string) => void }) {
  const [draft, setDraft] = useState(name);
  useEffect(() => setDraft(name), [name]);
  return (
    <input
      class="action-route-name-input"
      value={draft}
      onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
      onBlur={() => onCommit(draft)}
    />
  );
}

function WaypointSeconds({ value, onCommit }: { value: number; onCommit: (value: string) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  return (
    <input
      class="action-waypoint-seconds"
      type="number"
      min="0"
      step="0.5"
      value={draft}
      onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
      onBlur={() => onCommit(draft)}
      title="移動時間(秒)"
    />
  );
}

function RouteDetail({
  route,
  locations,
  onRename,
  onToggleLoop,
  onRun,
  onStop,
  onAddWaypoint,
  onUpdateWaypoint,
  onMoveWaypoint,
  onRemoveWaypoint,
}: {
  route: RouteEntry;
  locations: LocationEntry[];
  onRename: (name: string) => void;
  onToggleLoop: (loop: boolean) => void;
  onRun: () => void;
  onStop: () => void;
  onAddWaypoint: () => void;
  onUpdateWaypoint: (index: number, field: "location" | "seconds" | "wait", value: string | boolean) => void;
  onMoveWaypoint: (index: number, dir: -1 | 1) => void;
  onRemoveWaypoint: (index: number) => void;
}) {
  const waypoints = route.waypoints ?? [];
  return (
    <div class="action-route-detail">
      <div class="action-route-detail-head">
        <RouteNameInput name={route.name} onCommit={onRename} />
        <label class="action-route-loop-toggle">
          <input type="checkbox" checked={route.loop ?? false} onChange={(e) => onToggleLoop((e.target as HTMLInputElement).checked)} />
          ループ
        </label>
      </div>
      <div class="action-route-actions">
        <button type="button" class="btn btn-primary btn-small" onClick={onRun}>
          <Play size={13} />
          実行
        </button>
        <button type="button" class="btn btn-danger btn-small" onClick={onStop}>
          <Square size={13} />
          停止
        </button>
      </div>

      <div class="action-waypoint-list">
        {waypoints.length === 0 && <div class="action-waypoint-empty">経由地点がありません</div>}
        {waypoints.map((wp, i) => (
          <div class="action-waypoint-row" key={i}>
            <span class="action-waypoint-index">{i + 1}</span>
            <select
              class="action-waypoint-select"
              value={wp.location}
              onChange={(e) => onUpdateWaypoint(i, "location", (e.target as HTMLSelectElement).value)}
            >
              {locations.map((l) => (
                <option key={l.name} value={l.name}>
                  {l.name}
                </option>
              ))}
            </select>
            <WaypointSeconds value={wp.seconds ?? 3} onCommit={(v) => onUpdateWaypoint(i, "seconds", v)} />
            <label class="action-waypoint-wait">
              <input
                type="checkbox"
                checked={wp.wait ?? false}
                onChange={(e) => onUpdateWaypoint(i, "wait", (e.target as HTMLInputElement).checked)}
              />
              待機
            </label>
            <button type="button" class="icon-btn" disabled={i === 0} onClick={() => onMoveWaypoint(i, -1)} title="上へ">
              <ChevronUp size={14} />
            </button>
            <button
              type="button"
              class="icon-btn"
              disabled={i === waypoints.length - 1}
              onClick={() => onMoveWaypoint(i, 1)}
              title="下へ"
            >
              <ChevronDown size={14} />
            </button>
            <button type="button" class="icon-btn" onClick={() => onRemoveWaypoint(i)} title="削除">
              <Trash2 size={14} />
            </button>
          </div>
        ))}
        <button type="button" class="btn btn-ghost btn-small" onClick={onAddWaypoint} disabled={locations.length === 0}>
          <Plus size={14} />
          地点追加
        </button>
      </div>
    </div>
  );
}
