import { useEffect, useState } from "preact/hooks";
import { getMistRooms, getMistSync } from "../lib/api";
import type { MistLive } from "../lib/mist-status";

// Poll only while AI settings are mounted; never overlap slow CLI calls.
export function useMistStatus() {
  const [live, setLive] = useState<MistLive>({});
  useEffect(() => {
    let cancelled = false, timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      const [sync, rooms] = await Promise.allSettled([getMistSync(), getMistRooms()]);
      if (cancelled) return;
      setLive({ sync: sync.status === "fulfilled" ? sync.value : undefined,
        registration: rooms.status === "fulfilled" ? rooms.value.registrations.find(r => r.owner === "tc-npc") : undefined,
        error: rooms.status === "rejected" ? String(rooms.reason) : sync.status === "rejected" ? String(sync.reason) : undefined });
      timer = setTimeout(() => { void refresh(); }, 2000);
    }
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  }, []);
  return live;
}
