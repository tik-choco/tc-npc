import type { ProviderEntry } from "./config-types";
export interface MistSyncStatus {
  pending: boolean; applied: boolean; error: string | null; warnings: string[];
  updated_at: string | null; generation: number;
}
export interface MistRoomStatus { room: string; joined: boolean; providing: boolean; peers: number; models: string[] }
export interface MistRoom { room: string; consume: boolean; provide: boolean; shared: { provider_id: string; model: string }[] }
export interface MistRegistration { owner: string; rooms: MistRoom[]; updated_at?: string; status: { rooms: MistRoomStatus[] } }
export interface MistRooms { registrations: MistRegistration[] }
export interface MistLive { sync?: MistSyncStatus; registration?: MistRegistration; error?: string }
export function liveRoom(live: MistLive, provider: { base_url?: string }): MistRoomStatus | undefined {
  return live.registration?.status.rooms.find(r => r.room === provider.base_url?.replace(/^mist-network:\/\//, ""));
}
export function roomPhase(room: MistRoomStatus | undefined, error?: string): "ok" | "fetching" | "cache" | "error" {
  return error ? "error" : room?.joined ? "ok" : room ? "fetching" : "cache";
}
export function sharingApplied(live: MistLive, provider: ProviderEntry, providers: ProviderEntry[]): boolean {
  if (live.error || live.sync?.pending || live.sync?.error) return false;
  const applied = live.registration?.rooms.find(r => r.room === provider.base_url?.replace(/^mist-network:\/\//, ""));
  const refs = (provider.shared ?? []).filter(ref => providers.some(p => p.id === ref.provider_id && p.enabled !== false && /^https?:\/\//.test(p.base_url ?? "")) && ref.model.trim());
  return !!applied && applied.provide === (provider.provide ?? false) &&
    JSON.stringify(applied.shared) === JSON.stringify([...new Map(refs.map(ref => [JSON.stringify(ref), ref])).values()]);
}
