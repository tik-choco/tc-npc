// 視覚 tab: vision observation log (sense frames with kind:"vision") plus a
// read-only display of the relevant config fetched from /api/config.
import { useEffect, useRef, useState } from "preact/hooks";
import { Eye, Settings2 } from "lucide-preact";
import type { SenseEntry } from "../hooks/useNpcSocket";
import { getConfig } from "../lib/api";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/vision.css";

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(
    d.getSeconds(),
  ).padStart(2, "0")}`;
}

// The exact config shape is owned by the Rust side; this view just surfaces
// whatever top-level "vision" section exists (falling back to the full
// document) so the operator can sanity-check interval/etc. without a second
// schema to keep in sync.
function extractVisionConfig(config: ConfigDocument): unknown {
  if (config && typeof config === "object" && "vision" in config) {
    return (config as Record<string, unknown>).vision;
  }
  return config;
}

export function VisionView({ visionLog }: { visionLog: SenseEntry[] }) {
  const [config, setConfig] = useState<ConfigDocument | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    getConfig()
      .then(setConfig)
      .catch((err) => setConfigError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [visionLog.length]);

  return (
    <div class="vision-view">
      <section class="vision-config-card">
        <h2 class="vision-config-title">
          <Settings2 size={16} />
          視覚設定(読み取り専用)
        </h2>
        {configError && <div class="vision-config-error">読み込みに失敗しました: {configError}</div>}
        {!config && !configError && <div class="vision-config-loading">読み込み中…</div>}
        {config && <pre class="vision-config-pre">{JSON.stringify(extractVisionConfig(config), null, 2)}</pre>}
      </section>

      <section class="vision-log-section">
        <h2 class="vision-log-title">
          <Eye size={16} />
          観測ログ
        </h2>
        <div class="vision-log-scroll" ref={scrollRef}>
          {visionLog.length === 0 && (
            <div class="empty-state">
              <div class="empty-state-title">まだ観測がありません</div>
            </div>
          )}
          {visionLog.map((entry) => (
            <div key={entry.id} class="vision-log-item">
              <time>{formatTime(entry.ts)}</time>
              <span>{entry.text}</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
