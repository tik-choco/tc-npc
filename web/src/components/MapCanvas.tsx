// Self-contained SVG map for the 行動 (action) view: locations, routes, the
// NPC's live position + trail, pan/zoom, drag-to-move markers, and
// click-to-add-location. No external mapping library — VRChat worlds have no
// tile server to draw from, so this is just a metric grid with markers.
//
// Coordinate convention: `x`/`y` are the raw VRChat-world-local values coming
// off the WS `position` frame (meters). The SVG viewBox uses a flipped Y
// (svgY = -worldY) so that increasing world Y renders "up" on screen, with
// `heading` (degrees) treated as clockwise-from-up — matching VRChat's yaw
// convention when looking down at the horizontal plane.
import { useRef, useState } from "preact/hooks";
import { Check, MapPin, Maximize2, X, ZoomIn, ZoomOut } from "lucide-preact";
import type { LocationEntry, RouteEntry } from "../lib/config-types";
import type { PositionState } from "../hooks/useNpcSocket";
import { useI18n } from "../hooks/useI18n";
import "../styles/action.css";

interface ViewBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

const MIN_SPAN = 2;
const MAX_SPAN = 4000;
const DEFAULT_VIEWBOX: ViewBox = { x: -15, y: -15, w: 30, h: 30 };
const ROUTE_COLORS = ["#2563eb", "#059669", "#d946ef", "#ea580c", "#0891b2", "#7c3aed"];
const DRAG_THRESHOLD = 4;

function clamp(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n));
}

function round2(n: number): number {
  return Math.round(n * 100) / 100;
}

function worldToSvg(x: number, y: number): { x: number; y: number } {
  return { x, y: -y };
}

function svgToWorld(x: number, y: number): { x: number; y: number } {
  return { x, y: -y };
}

function clientToSvgPoint(svg: SVGSVGElement, clientX: number, clientY: number): { x: number; y: number } {
  const ctm = svg.getScreenCTM();
  if (!ctm) return { x: 0, y: 0 };
  const pt = svg.createSVGPoint();
  pt.x = clientX;
  pt.y = clientY;
  const p = pt.matrixTransform(ctm.inverse());
  return { x: p.x, y: p.y };
}

function svgPointToClient(svg: SVGSVGElement, x: number, y: number): { x: number; y: number } {
  const ctm = svg.getScreenCTM();
  if (!ctm) return { x: 0, y: 0 };
  const pt = svg.createSVGPoint();
  pt.x = x;
  pt.y = y;
  const p = pt.matrixTransform(ctm);
  return { x: p.x, y: p.y };
}

function clientToWorld(svg: SVGSVGElement, clientX: number, clientY: number): { x: number; y: number } {
  const p = clientToSvgPoint(svg, clientX, clientY);
  return svgToWorld(p.x, p.y);
}

function worldToClient(svg: SVGSVGElement, x: number, y: number): { x: number; y: number } {
  const s = worldToSvg(x, y);
  return svgPointToClient(svg, s.x, s.y);
}

function zoomViewBox(vb: ViewBox, anchor: { x: number; y: number }, factor: number): ViewBox {
  const newW = clamp(vb.w * factor, MIN_SPAN, MAX_SPAN);
  const ratio = newW / vb.w;
  const newH = vb.h * ratio;
  const relX = (anchor.x - vb.x) / vb.w;
  const relY = (anchor.y - vb.y) / vb.h;
  return { x: anchor.x - relX * newW, y: anchor.y - relY * newH, w: newW, h: newH };
}

function gridStep(span: number): { minor: number; major: number } {
  const steps = [0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000];
  const target = span / 10;
  let minor = steps[steps.length - 1];
  for (const s of steps) {
    if (s >= target) {
      minor = s;
      break;
    }
  }
  return { minor, major: minor * 5 };
}

function nextName(base: string, existing: Set<string>): string {
  if (!existing.has(base)) return base;
  let i = 2;
  while (existing.has(`${base}${i}`)) i++;
  return `${base}${i}`;
}

export interface MapCanvasProps {
  locations: LocationEntry[];
  routes: RouteEntry[];
  selectedRouteName: string | null;
  position: PositionState | null;
  trail: { x: number; y: number }[];
  addMode: boolean;
  onToggleAddMode: () => void;
  onAddLocation: (name: string, x: number, y: number) => void;
  onMoveLocation: (name: string, x: number, y: number) => void;
}

export function MapCanvas({
  locations,
  routes,
  selectedRouteName,
  position,
  trail,
  addMode,
  onToggleAddMode,
  onAddLocation,
  onMoveLocation,
}: MapCanvasProps) {
  const { t } = useI18n();
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [viewBox, setViewBox] = useState<ViewBox>(DEFAULT_VIEWBOX);
  const [hoverWorld, setHoverWorld] = useState<{ x: number; y: number } | null>(null);
  const [dragLoc, setDragLoc] = useState<{ name: string; x: number; y: number } | null>(null);
  const [pendingAdd, setPendingAdd] = useState<{ x: number; y: number; name: string } | null>(null);

  const panRef = useRef<{
    pointerId: number;
    startClientX: number;
    startClientY: number;
    last: { x: number; y: number };
    moved: boolean;
  } | null>(null);
  const dragLocRef = useRef<{ name: string; pointerId: number } | null>(null);

  const locByName = new Map(locations.map((l) => [l.name, l]));

  function zoomAtCenter(factor: number) {
    setPendingAdd(null);
    setViewBox((vb) => zoomViewBox(vb, { x: vb.x + vb.w / 2, y: vb.y + vb.h / 2 }, factor));
  }

  function handleWheel(e: WheelEvent) {
    e.preventDefault();
    const svg = svgRef.current;
    if (!svg) return;
    setPendingAdd(null);
    const p = clientToSvgPoint(svg, e.clientX, e.clientY);
    const factor = e.deltaY > 0 ? 1.15 : 1 / 1.15;
    setViewBox((vb) => zoomViewBox(vb, p, factor));
  }

  function fitBounds() {
    setPendingAdd(null);
    const pts: { x: number; y: number }[] = locations.map((l) => ({ x: l.x ?? 0, y: l.y ?? 0 }));
    if (position) pts.push({ x: position.x, y: position.y });
    if (pts.length === 0) {
      setViewBox(DEFAULT_VIEWBOX);
      return;
    }
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    for (const p of pts) {
      minX = Math.min(minX, p.x);
      maxX = Math.max(maxX, p.x);
      minY = Math.min(minY, p.y);
      maxY = Math.max(maxY, p.y);
    }
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const span = clamp(Math.max(maxX - minX, maxY - minY) * 1.4, 8, MAX_SPAN);
    setViewBox({ x: cx - span / 2, y: -(cy + span / 2), w: span, h: span });
  }

  function handleBgPointerDown(e: PointerEvent) {
    const svg = svgRef.current;
    if (!svg) return;
    svg.setPointerCapture(e.pointerId);
    const start = clientToSvgPoint(svg, e.clientX, e.clientY);
    panRef.current = { pointerId: e.pointerId, startClientX: e.clientX, startClientY: e.clientY, last: start, moved: false };
  }

  function handleBgPointerMove(e: PointerEvent) {
    const svg = svgRef.current;
    if (!svg) return;
    setHoverWorld(clientToWorld(svg, e.clientX, e.clientY));

    const pan = panRef.current;
    if (!pan || pan.pointerId !== e.pointerId) return;
    const dxPx = e.clientX - pan.startClientX;
    const dyPx = e.clientY - pan.startClientY;
    if (!pan.moved && Math.hypot(dxPx, dyPx) > DRAG_THRESHOLD) {
      pan.moved = true;
      setPendingAdd(null);
    }
    if (!pan.moved) return;
    const cur = clientToSvgPoint(svg, e.clientX, e.clientY);
    const dx = cur.x - pan.last.x;
    const dy = cur.y - pan.last.y;
    pan.last = cur;
    setViewBox((vb) => ({ ...vb, x: vb.x - dx, y: vb.y - dy }));
  }

  function handleBgPointerUp(e: PointerEvent) {
    const pan = panRef.current;
    if (!pan || pan.pointerId !== e.pointerId) return;
    const svg = svgRef.current;
    const wasClick = !pan.moved;
    panRef.current = null;
    if (wasClick && addMode && svg) {
      const world = clientToWorld(svg, e.clientX, e.clientY);
      const used = new Set(locations.map((l) => l.name));
      setPendingAdd({
        x: round2(world.x),
        y: round2(world.y),
        name: nextName(t("action.loc.defaultName"), used),
      });
    }
  }

  function handleMarkerPointerDown(e: PointerEvent, loc: LocationEntry) {
    e.stopPropagation();
    const target = e.currentTarget as SVGElement | null;
    target?.setPointerCapture(e.pointerId);
    dragLocRef.current = { name: loc.name, pointerId: e.pointerId };
    setDragLoc({ name: loc.name, x: loc.x ?? 0, y: loc.y ?? 0 });
  }

  function handleMarkerPointerMove(e: PointerEvent) {
    e.stopPropagation();
    const drag = dragLocRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    const svg = svgRef.current;
    if (!svg) return;
    const world = clientToWorld(svg, e.clientX, e.clientY);
    setDragLoc({ name: drag.name, x: world.x, y: world.y });
  }

  function handleMarkerPointerUp(e: PointerEvent) {
    e.stopPropagation();
    const drag = dragLocRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    dragLocRef.current = null;
    const svg = svgRef.current;
    setDragLoc(null);
    if (!svg) return;
    const world = clientToWorld(svg, e.clientX, e.clientY);
    const original = locByName.get(drag.name);
    const ox = original?.x ?? 0;
    const oy = original?.y ?? 0;
    if (Math.abs(world.x - ox) > 0.005 || Math.abs(world.y - oy) > 0.005) {
      onMoveLocation(drag.name, round2(world.x), round2(world.y));
    }
  }

  function confirmAdd() {
    if (!pendingAdd) return;
    onAddLocation(pendingAdd.name.trim() || t("action.loc.defaultName"), pendingAdd.x, pendingAdd.y);
    setPendingAdd(null);
  }

  const { minor, major } = gridStep(viewBox.w);
  const startXi = Math.floor(viewBox.x / minor);
  const endXi = Math.ceil((viewBox.x + viewBox.w) / minor);
  const vLines: { key: string; v: number; strong: boolean }[] = [];
  for (let i = startXi; i <= endXi; i++) {
    const gx = i * minor;
    vLines.push({ key: `v${i}`, v: gx, strong: Math.abs(gx % major) < minor / 2 });
  }
  const startYi = Math.floor(viewBox.y / minor);
  const endYi = Math.ceil((viewBox.y + viewBox.h) / minor);
  const hLines: { key: string; v: number; strong: boolean }[] = [];
  for (let i = startYi; i <= endYi; i++) {
    const gy = i * minor;
    hLines.push({ key: `h${i}`, v: gy, strong: Math.abs(gy % major) < minor / 2 });
  }

  let popoverStyle: { left: string; top: string } | null = null;
  if (pendingAdd && svgRef.current && wrapperRef.current) {
    const client = worldToClient(svgRef.current, pendingAdd.x, pendingAdd.y);
    const rect = wrapperRef.current.getBoundingClientRect();
    popoverStyle = { left: `${client.x - rect.left}px`, top: `${client.y - rect.top}px` };
  }

  const trailPoints = trail.map((p) => worldToSvg(p.x, p.y));
  const trailAttr = trailPoints.map((p) => `${p.x},${p.y}`).join(" ");

  let headingArrow: { tip: { x: number; y: number }; baseL: { x: number; y: number }; baseR: { x: number; y: number } } | null =
    null;
  let posSvg: { x: number; y: number } | null = null;
  if (position) {
    posSvg = worldToSvg(position.x, position.y);
    const rad = (position.heading * Math.PI) / 180;
    const dir = { x: Math.sin(rad), y: -Math.cos(rad) };
    const perp = { x: -dir.y, y: dir.x };
    headingArrow = {
      tip: { x: posSvg.x + dir.x * 1.4, y: posSvg.y + dir.y * 1.4 },
      baseL: { x: posSvg.x + dir.x * 0.5 + perp.x * 0.4, y: posSvg.y + dir.y * 0.5 + perp.y * 0.4 },
      baseR: { x: posSvg.x + dir.x * 0.5 - perp.x * 0.4, y: posSvg.y + dir.y * 0.5 - perp.y * 0.4 },
    };
  }

  return (
    <div class="map-canvas-wrap" ref={wrapperRef}>
      <svg
        ref={svgRef}
        class={`map-canvas-svg${addMode ? " is-add-mode" : ""}`}
        viewBox={`${viewBox.x} ${viewBox.y} ${viewBox.w} ${viewBox.h}`}
        onWheel={(e) => handleWheel(e)}
        onPointerDown={(e) => handleBgPointerDown(e)}
        onPointerMove={(e) => handleBgPointerMove(e)}
        onPointerUp={(e) => handleBgPointerUp(e)}
        onPointerLeave={() => setHoverWorld(null)}
      >
        <g class="map-grid">
          {vLines.map((l) => (
            <line key={l.key} x1={l.v} y1={viewBox.y} x2={l.v} y2={viewBox.y + viewBox.h} class={l.strong ? "grid-major" : "grid-minor"} />
          ))}
          {hLines.map((l) => (
            <line key={l.key} x1={viewBox.x} y1={l.v} x2={viewBox.x + viewBox.w} y2={l.v} class={l.strong ? "grid-major" : "grid-minor"} />
          ))}
          <line class="grid-axis" x1={viewBox.x} y1={0} x2={viewBox.x + viewBox.w} y2={0} />
          <line class="grid-axis" x1={0} y1={viewBox.y} x2={0} y2={viewBox.y + viewBox.h} />
        </g>

        <g class="map-routes">
          {routes.map((route, idx) => {
            const pts = (route.waypoints ?? [])
              .map((w) => locByName.get(w.location))
              .filter((l): l is LocationEntry => Boolean(l))
              .map((l) => worldToSvg(l.x ?? 0, l.y ?? 0));
            if (pts.length < 2) return null;
            const isSelected = route.name === selectedRouteName;
            const color = ROUTE_COLORS[idx % ROUTE_COLORS.length];
            const pointsAttr = pts.map((p) => `${p.x},${p.y}`).join(" ");
            return (
              <g key={route.name}>
                <polyline
                  points={pointsAttr}
                  fill="none"
                  stroke={color}
                  strokeWidth={isSelected ? 0.3 : 0.16}
                  strokeOpacity={isSelected ? 0.95 : 0.4}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
                {route.loop && (
                  <line
                    x1={pts[pts.length - 1].x}
                    y1={pts[pts.length - 1].y}
                    x2={pts[0].x}
                    y2={pts[0].y}
                    stroke={color}
                    strokeWidth={isSelected ? 0.3 : 0.16}
                    strokeOpacity={isSelected ? 0.95 : 0.4}
                    strokeDasharray="0.5 0.35"
                  />
                )}
              </g>
            );
          })}
        </g>

        {trailPoints.length > 1 && (
          <polyline points={trailAttr} fill="none" class="map-trail" />
        )}

        <g class="map-locations">
          {locations.map((loc) => {
            const isDragging = dragLoc?.name === loc.name;
            const p = worldToSvg(isDragging ? dragLoc!.x : loc.x ?? 0, isDragging ? dragLoc!.y : loc.y ?? 0);
            return (
              <g
                key={loc.name}
                class={`map-location${isDragging ? " is-dragging" : ""}`}
                onPointerDown={(e) => handleMarkerPointerDown(e, loc)}
                onPointerMove={(e) => handleMarkerPointerMove(e)}
                onPointerUp={(e) => handleMarkerPointerUp(e)}
              >
                <circle cx={p.x} cy={p.y} r={1.1} class="map-location-hit" />
                <circle cx={p.x} cy={p.y} r={0.5} class="map-location-dot" />
                <text x={p.x} y={p.y - 0.75} class="map-location-label">
                  {loc.name}
                </text>
              </g>
            );
          })}
        </g>

        {posSvg && headingArrow && (
          <g class="map-position">
            {trailPoints.length > 0 && <circle cx={posSvg.x} cy={posSvg.y} r={0.9} class="map-position-ring" />}
            <polygon
              points={`${headingArrow.tip.x},${headingArrow.tip.y} ${headingArrow.baseL.x},${headingArrow.baseL.y} ${headingArrow.baseR.x},${headingArrow.baseR.y}`}
              class="map-position-arrow"
            />
            <circle cx={posSvg.x} cy={posSvg.y} r={0.55} class="map-position-dot" />
          </g>
        )}
      </svg>

      {pendingAdd && popoverStyle && (
        <div class="map-add-popover" style={popoverStyle}>
          <input
            autoFocus
            value={pendingAdd.name}
            onInput={(e) => setPendingAdd({ ...pendingAdd, name: (e.target as HTMLInputElement).value })}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                confirmAdd();
              } else if (e.key === "Escape") {
                setPendingAdd(null);
              }
            }}
          />
          <button type="button" class="icon-btn" onClick={confirmAdd} title={t("common.add")}>
            <Check size={14} />
          </button>
          <button type="button" class="icon-btn" onClick={() => setPendingAdd(null)} title={t("common.cancel")}>
            <X size={14} />
          </button>
        </div>
      )}

      <div class="map-toolbar">
        <button type="button" class="icon-btn" onClick={() => zoomAtCenter(1 / 1.4)} title={t("map.zoomIn")}>
          <ZoomIn size={16} />
        </button>
        <button type="button" class="icon-btn" onClick={() => zoomAtCenter(1.4)} title={t("map.zoomOut")}>
          <ZoomOut size={16} />
        </button>
        <button type="button" class="icon-btn" onClick={fitBounds} title={t("map.fit")}>
          <Maximize2 size={16} />
        </button>
        <button
          type="button"
          class={`icon-btn${addMode ? " is-active" : ""}`}
          onClick={onToggleAddMode}
          title={t("map.addMode")}
        >
          <MapPin size={16} />
        </button>
      </div>

      <div class="map-readout">
        {hoverWorld
          ? `X ${hoverWorld.x.toFixed(1)} / Y ${hoverWorld.y.toFixed(1)}`
          : t("map.range", { meters: Math.round(viewBox.w) })}
      </div>

      {addMode && <div class="map-hint">{t("map.hint")}</div>}
    </div>
  );
}
