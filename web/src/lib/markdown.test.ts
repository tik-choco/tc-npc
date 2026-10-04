import { describe, expect, it } from "vitest";

import { flattenBlocks, parseInline, parseInlinePreview, parseMarkdown } from "./markdown";

/** The text of an inline run, delimiters dropped — what a reader sees. */
function textOf(nodes: ReturnType<typeof parseInline>): string {
  return nodes
    .map((node) => {
      switch (node.kind) {
        case "text":
        case "code":
          return node.text;
        default:
          return textOf(node.children);
      }
    })
    .join("");
}

describe("parseInline", () => {
  it("turns ** into strong instead of leaving the asterisks in the text", () => {
    // The bug that prompted all of this: memory documents are LLM-written and
    // arrive full of **...**, which the transcript printed verbatim.
    expect(parseInline("これは**重要**です")).toEqual([
      { kind: "text", text: "これは" },
      { kind: "strong", children: [{ kind: "text", text: "重要" }] },
      { kind: "text", text: "です" },
    ]);
  });

  it("reads * as emphasis and nests it inside **", () => {
    expect(parseInline("*a*")).toEqual([{ kind: "em", children: [{ kind: "text", text: "a" }] }]);
    expect(parseInline("**bold *and italic***")).toEqual([
      {
        kind: "strong",
        children: [
          { kind: "text", text: "bold " },
          { kind: "em", children: [{ kind: "text", text: "and italic" }] },
        ],
      },
    ]);
  });

  it("takes the shortest run, so two bold spans on a line stay two", () => {
    const spans = parseInline("**a** と **b**");
    expect(spans.map((s) => s.kind)).toEqual(["strong", "text", "strong"]);
    expect(textOf(spans)).toBe("a と b");
  });

  it("keeps code literal, so backticked asterisks are not emphasis", () => {
    expect(parseInline("`**x**`")).toEqual([{ kind: "code", text: "**x**" }]);
  });

  it("leaves underscores alone", () => {
    // snake_case shows up constantly in memory about config; treating _ as
    // emphasis would italicize half of such a line.
    expect(parseInline("source_language と target_language")).toEqual([
      { kind: "text", text: "source_language と target_language" },
    ]);
  });

  it("leaves a lone or unclosed marker as text", () => {
    expect(parseInline("2 * 3 = 6")).toEqual([{ kind: "text", text: "2 * 3 = 6" }]);
    expect(parseInline("**unclosed")).toEqual([{ kind: "text", text: "**unclosed" }]);
  });

  it("links only http/https/mailto, and prints anything else literally", () => {
    expect(parseInline("[docs](https://example.com/a)")).toEqual([
      { kind: "link", href: "https://example.com/a", children: [{ kind: "text", text: "docs" }] },
    ]);
    const unsafe = "[x](javascript:alert(1))";
    expect(parseInline(unsafe)).toEqual([{ kind: "text", text: unsafe }]);
  });
});

describe("parseMarkdown", () => {
  it("groups consecutive bullets into one list", () => {
    const blocks = parseMarkdown("- 一つ目\n- 二つ目\n- 三つ目");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ kind: "list", ordered: false });
    expect(blocks[0].kind === "list" && blocks[0].items.map(textOf)).toEqual(["一つ目", "二つ目", "三つ目"]);
  });

  it("separates an ordered list from an unordered one", () => {
    const blocks = parseMarkdown("- a\n1. b");
    expect(blocks.map((b) => b.kind)).toEqual(["list", "list"]);
    expect(blocks[0]).toMatchObject({ ordered: false });
    expect(blocks[1]).toMatchObject({ ordered: true });
  });

  it("reads headings with their level and formats the heading text", () => {
    const blocks = parseMarkdown("## **強調**した見出し");
    expect(blocks[0]).toMatchObject({ kind: "heading", level: 2 });
    expect(blocks[0].kind === "heading" && blocks[0].spans[0].kind).toBe("strong");
  });

  it("joins the lines of a paragraph with a space and splits on a blank line", () => {
    // Matches what the transcript already did with a multi-line memory: the
    // text went into HTML, where a newline is whitespace.
    const blocks = parseMarkdown("one\ntwo\n\nthree");
    expect(blocks.map((b) => b.kind === "paragraph" && textOf(b.spans))).toEqual(["one two", "three"]);
  });

  it("keeps a fenced block verbatim, including its asterisks and newlines", () => {
    expect(parseMarkdown("```\n**a**\nb\n```")).toEqual([{ kind: "codeBlock", text: "**a**\nb" }]);
  });

  it("takes the rest of the document as code when a fence is never closed", () => {
    // A model that ran out of tokens mid-block shouldn't cost us the text.
    expect(parseMarkdown("```\nstill code")).toEqual([{ kind: "codeBlock", text: "still code" }]);
  });

  it("ends a list at the first plain line under it", () => {
    const blocks = parseMarkdown("- a\n- b\nplain");
    expect(blocks.map((b) => b.kind)).toEqual(["list", "paragraph"]);
  });

  it("returns nothing for empty or whitespace-only input", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(parseMarkdown("   \n\n  ")).toEqual([]);
  });
});

describe("parseInlinePreview", () => {
  it("flattens a document to one run while keeping its formatting", () => {
    const spans = parseInlinePreview("# 見出し\n\n- **一つ目**\n- 二つ目");
    expect(textOf(spans)).toBe("見出し ・一つ目 ・二つ目");
    expect(spans.some((s) => s.kind === "strong")).toBe(true);
  });

  it("is empty for an empty document", () => {
    expect(flattenBlocks([])).toEqual([]);
    expect(parseInlinePreview("")).toEqual([]);
  });
});
