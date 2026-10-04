import { describe, expect, it, vi, afterEach } from "vitest";
import { liveRoom, roomPhase, sharingApplied, type MistLive } from "./mist-status";
import { MIST_MESSAGES, AI_MESSAGES } from "./ai-messages";
import { getMistRooms, getMistSync } from "./api";
import type { ProviderEntry } from "./config-types";

const providers: ProviderEntry[] = [
  { id: "h", label: "HTTP", base_url: "http://endpoint", enabled: true },
  { id: "off", label: "Disabled", base_url: "http://disabled", enabled: false },
  { id: "r", label: "Room", base_url: "mist-network://team", provide: true,
    shared: [{ provider_id: "h", model: "model" }, { provider_id: "off", model: "ignored" }] },
];
const live: MistLive = { registration: { owner: "tc-npc", rooms: [{ room: "team", consume: true, provide: true, shared: [{ provider_id: "h", model: "model" }] }],
  status: { rooms: [{ room: "team", joined: true, providing: true, peers: 2, models: ["model"] }] } } };
afterEach(() => vi.unstubAllGlobals());
describe("live mistl state", () => {
  it("uses the provider's room rather than another room", () => {
    expect(liveRoom(live, providers[2]!)).toMatchObject({ joined: true, providing: true, peers: 2 });
    expect(liveRoom(live, { base_url: "mist-network://other" })).toBeUndefined();
    expect(roomPhase(liveRoom(live, providers[2]!))).toBe("ok");
    expect(roomPhase(undefined)).toBe("cache");
    expect(roomPhase({ room: "team", joined: false, providing: false, peers: 0, models: [] })).toBe("fetching");
    expect(roomPhase(undefined, "failed")).toBe("error");
  });
  it("distinguishes applied refs from pending, failed and edited settings", () => {
    expect(sharingApplied(live, providers[2]!, providers)).toBe(true);
    expect(sharingApplied(live, { ...providers[2]!, provide: false }, providers)).toBe(false);
    expect(sharingApplied(live, { ...providers[2]!, shared: [] }, providers)).toBe(false);
    expect(sharingApplied({ ...live, error: "offline" }, providers[2]!, providers)).toBe(false);
    expect(sharingApplied({ ...live, sync: { pending: true, applied: true, error: null, warnings: [], updated_at: null, generation: 1 } }, providers[2]!, providers)).toBe(false);
  });
  it("calls only the server's status REST endpoints", async () => {
    const fetch = vi.fn().mockResolvedValue({ ok: true, status: 200, text: async () => JSON.stringify({ registrations: [] }) });
    vi.stubGlobal("fetch", fetch);
    await getMistRooms(); await getMistSync();
    expect(fetch.mock.calls.map(call => call[0])).toEqual(["/api/mist/rooms", "/api/mist/sync"]);
  });
});
describe("AI locale completeness", () => {
  for (const messages of [MIST_MESSAGES, AI_MESSAGES]) for (const locale of ["ja", "zh-CN", "zh-TW"] as const) {
    it(`has matching keys and placeholders in ${locale}`, () => {
      expect(Object.keys(messages[locale]).sort()).toEqual(Object.keys(messages.en).sort());
      for (const key of Object.keys(messages.en) as (keyof typeof messages.en)[]) {
        const value = messages[locale][key];
        expect(value.trim()).not.toBe("");
        expect(value.match(/\{\w+\}/g) ?? []).toEqual(messages.en[key].match(/\{\w+\}/g) ?? []);
      }
    });
  }
});
