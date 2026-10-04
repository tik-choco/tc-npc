import { describe, expect, it } from "vitest";
import { LLM_SETTINGS_MESSAGES, REASONING_EFFORT_OPTIONS } from "@tik-choco/mistai/preact";
import { AI_MESSAGES } from "../lib/ai-messages";

function placeholders(value: string) { return [...value.matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort(); }
describe("AI settings vocabulary", () => {
  it("offers every supported reasoning effort including explicit none and max", () => {
    expect(REASONING_EFFORT_OPTIONS).toEqual(["none", "minimal", "low", "medium", "high", "xhigh", "max"]);
  });
  for (const catalogs of [LLM_SETTINGS_MESSAGES, AI_MESSAGES]) {
    for (const locale of ["ja", "zh-CN", "zh-TW"] as const) {
      it(`has complete nonempty ${locale} messages and matching placeholders`, () => {
        const base = catalogs.en as Record<string, string>, target = catalogs[locale] as Record<string, string>;
        expect(Object.keys(target).sort()).toEqual(Object.keys(base).sort());
        for (const key of Object.keys(base)) { expect(target[key]?.trim(), key).toBeTruthy(); expect(placeholders(target[key]!)).toEqual(placeholders(base[key]!)); }
      });
    }
  }
});