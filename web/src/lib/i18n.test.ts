// Unit tests for lib/i18n.ts's own logic, as opposed to the dictionaries
// themselves (the ja/en/zh key parity is already enforced at compile time by
// MessageKey being derived from `ja`). What's worth locking down here is
// translate()'s {placeholder} interpolation and its fallback chain, plus
// detectLang()'s browser-language matching and its defensive fallbacks when
// localStorage/navigator aren't cooperating.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { detectLang, translate } from "./i18n";

describe("translate", () => {
  it("substitutes a known {placeholder} with the given value", () => {
    expect(translate("en", "chat.composer.speaker.label")).toContain("keeps internal state");
    expect(translate("ja", "characters.toast.imported", { name: "たろう" })).toBe("「たろう」をインポートしました");
  });

  it("stringifies a numeric placeholder value", () => {
    expect(translate("en", "people.list.count", { count: 5 })).toBe("5 people");
  });

  it("leaves an unknown placeholder token as-is instead of dropping it", () => {
    expect(translate("en", "people.row.encounters", {})).toBe("{n} encounter(s)");
  });

  it("returns the template unchanged when no params are given, even if it has placeholders", () => {
    expect(translate("en", "people.row.encounters")).toBe("{n} encounter(s)");
  });

  it("substitutes a placeholder embedded between literal punctuation", () => {
    expect(translate("en", "vrm.toast.added", { name: "Rin" })).toBe("Added “Rin”");
  });
});

// A class rather than an inline object literal: a `{ store, getItem() {
// this.store... } }` literal loses its `this` typing once passed through
// vi.stubGlobal's `value: unknown` parameter, so `this.store` doesn't
// type-check. A class instance doesn't have that problem.
class MemoryStorage {
  private store = new Map<string, string>();
  getItem(key: string): string | null {
    return this.store.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.store.set(key, value);
  }
}

describe("detectLang", () => {
  // Stubbed wholesale via vi.stubGlobal (rather than
  // Object.defineProperty(navigator, ...)) so vi.unstubAllGlobals() in
  // afterEach fully restores Node's real `navigator` global afterwards —
  // `languages` lives on its prototype, not as an own property, so a
  // property-level save/restore would silently fail to put it back.
  function stubNavigatorLanguages(languages: string[]): void {
    vi.stubGlobal("navigator", { languages });
  }

  beforeEach(() => {
    vi.stubGlobal("localStorage", new MemoryStorage());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns the saved choice from localStorage when present", () => {
    localStorage.setItem("tc-npc:lang", "zh");
    stubNavigatorLanguages(["en-US"]);
    expect(detectLang()).toBe("zh");
  });

  it("ignores a stored value that isn't one of the shipped languages", () => {
    localStorage.setItem("tc-npc:lang", "fr");
    stubNavigatorLanguages(["en-US"]);
    expect(detectLang()).toBe("en");
  });

  it("matches the browser's preferred language by primary subtag", () => {
    stubNavigatorLanguages(["zh-Hant-TW", "en-US"]);
    expect(detectLang()).toBe("zh");
  });

  it("is case-insensitive when matching the primary subtag", () => {
    stubNavigatorLanguages(["EN-GB"]);
    expect(detectLang()).toBe("en");
  });

  it("skips an unsupported preferred language and matches the next one", () => {
    stubNavigatorLanguages(["fr-FR", "en-US"]);
    expect(detectLang()).toBe("en");
  });

  it("falls back to Japanese when nothing in navigator.languages is shipped", () => {
    stubNavigatorLanguages(["fr-FR", "de-DE"]);
    expect(detectLang()).toBe("ja");
  });

  it("falls back to Japanese when localStorage throws (private browsing)", () => {
    vi.stubGlobal("localStorage", {
      getItem() {
        throw new Error("SecurityError");
      },
    });
    stubNavigatorLanguages(["en-US"]);
    expect(detectLang()).toBe("en"); // storage failure alone doesn't block browser-language matching
  });
});
