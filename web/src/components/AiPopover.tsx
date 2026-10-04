import type { ComponentChildren, RefObject } from "preact";
import { createPortal } from "preact/compat";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";

// Fixed overlays never change the settings height. Dismiss at pointerdown,
// so drags that begin inside and end outside cannot dismiss a popup.
export function AiPopover({ anchor, close, children, wide = false }: {
  anchor: RefObject<HTMLElement>; close(): void; children: ComponentChildren; wide?: boolean;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ top: 0, left: 0, width: 0, maxHeight: 420, up: false });
  useLayoutEffect(() => {
    function move() {
      const rect = anchor.current?.getBoundingClientRect();
      if (!rect) return;
      const width = Math.min(wide ? 640 : 350, innerWidth - 32);
      const height = Math.min(420, innerHeight - 32);
      const up = innerHeight - rect.bottom < height && rect.top > innerHeight - rect.bottom;
      setPosition({ top: up ? Math.max(16, rect.top - height - 6) : Math.min(rect.bottom + 6, innerHeight - height - 16),
        left: Math.max(16, Math.min(rect.left, innerWidth - width - 16)), width, maxHeight: height, up });
    }
    move();
    addEventListener("resize", move); addEventListener("scroll", move, true);
    return () => { removeEventListener("resize", move); removeEventListener("scroll", move, true); };
  }, [anchor, wide]);
  useEffect(() => {
    panel.current?.querySelector<HTMLElement>("input, button")?.focus({ preventScroll: true });
    function outside(event: PointerEvent) {
      if (!panel.current?.contains(event.target as Node) && !anchor.current?.contains(event.target as Node)) close();
    }
    function key(event: KeyboardEvent) { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } }
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", key);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", key); anchor.current?.focus({ preventScroll: true }); };
  }, []);
  return createPortal(<div ref={panel} class="npc-ai npc-ai-popover" style={{ top: position.top, left: position.left,
    width: position.width, maxHeight: position.maxHeight, transformOrigin: position.up ? "bottom left" : "top left" }}>
    {children}
  </div>, document.body);
}