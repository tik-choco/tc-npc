// A very small Markdown subset, parsed to a data structure rather than to
// HTML.
//
// Why this exists at all: memory writes are LLM-authored documents (see
// npc-memory), and models write them in Markdown whether or not anyone asked
// — `**強調**`, `- ` bullets, the occasional `#` heading. The チャット
// transcript used to print that verbatim, so the asterisks showed up as
// asterisks. This turns them back into formatting.
//
// Why not a Markdown library: the two things one buys are a full CommonMark
// implementation and an HTML sanitizer, and neither is wanted here. Rendering
// goes through components/Markdown.tsx, which builds VNodes — the produced
// tree is `strong`/`em`/`code`/`a` and nothing else, so there is no
// innerHTML anywhere and no sanitizer to keep honest. Model-authored text is
// exactly the sort of input that shouldn't be handed to a HTML pipeline.
//
// Why a data structure rather than VNodes directly: it keeps this file a pure
// function that a plain unit test can assert on (see markdown.test.ts),
// matching how the rest of lib/ is split from the components that render it.
//
// Deliberately NOT supported, because none of it appears in what this renders
// and each carries a cost: `_underscore_` emphasis (`snake_case` identifiers
// are common in memory about config, and would emphasize half a line),
// blockquotes, tables, images, reference links, HTML passthrough, and setext
// headings. Anything unrecognized stays literal text, which is the same thing
// the transcript did before this module existed.

/** A run of formatted text. `children` nests, so `**bold *and italic***`
 *  parses rather than being flattened to one or the other. */
export type Inline =
  | { kind: "text"; text: string }
  | { kind: "strong"; children: Inline[] }
  | { kind: "em"; children: Inline[] }
  | { kind: "code"; text: string }
  | { kind: "link"; href: string; children: Inline[] };

/** A top-level block. `list` keeps its items together so the renderer can
 *  emit one `<ul>`/`<ol>` per run rather than one per line. */
export type Block =
  | { kind: "paragraph"; spans: Inline[] }
  | { kind: "heading"; level: number; spans: Inline[] }
  | { kind: "list"; ordered: boolean; items: Inline[][] }
  | { kind: "codeBlock"; text: string };

/** Inline constructs, tried in this order at each position. `code` comes
 *  first because its content is literal (backticked `**` is not emphasis),
 *  and `**` before `*` so bold isn't read as two empty italics. */
const INLINE_PATTERN = new RegExp(
  [
    "`([^`\\n]+)`", // 1: code
    "\\[([^\\]\\n]+)\\]\\(([^()\\s]+)\\)", // 2: link text, 3: href
    // The `(?!\*)` on the close is what makes `**bold *and italic***` end at
    // the *last* asterisk: without it the shortest close wins, the run ends
    // one asterisk early, and the italic inside it is left dangling.
    "\\*\\*(?=\\S)([\\s\\S]*?\\S)\\*\\*(?!\\*)", // 4: strong
    "\\*(?=\\S)([^*\\n]*?\\S)\\*", // 5: em
  ].join("|"),
);

/** Schemes a `[text](href)` link is allowed to carry. Everything else — most
 *  pointedly `javascript:` — falls back to rendering the link as the literal
 *  text the model wrote, so an href is never handed to the browser unchecked
 *  just because it was formatted like a link. */
const SAFE_HREF = /^(https?:\/\/|mailto:)/i;

const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const ORDERED = /^\s*\d+[.)]\s+(.*)$/;
const FENCE = /^\s*```/;

/** Parses one line's worth of inline formatting. Recurses into the content of
 *  `**`/`*`/link runs; every recursion strips at least the delimiters, so the
 *  input strictly shrinks and this terminates. */
export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let rest = src;

  const pushText = (text: string) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last?.kind === "text") last.text += text;
    else out.push({ kind: "text", text });
  };

  while (rest) {
    const match = INLINE_PATTERN.exec(rest);
    if (!match || match.index === undefined) break;

    pushText(rest.slice(0, match.index));
    const [whole, code, linkText, href, strong, em] = match;

    if (code !== undefined) {
      out.push({ kind: "code", text: code });
    } else if (linkText !== undefined && href !== undefined) {
      if (SAFE_HREF.test(href)) out.push({ kind: "link", href, children: parseInline(linkText) });
      else pushText(whole);
    } else if (strong !== undefined) {
      out.push({ kind: "strong", children: parseInline(strong) });
    } else if (em !== undefined) {
      out.push({ kind: "em", children: parseInline(em) });
    }

    rest = rest.slice(match.index + whole.length);
  }

  pushText(rest);
  return out;
}

/**
 * Parses a document into blocks.
 *
 * Paragraph lines are joined with a space rather than kept as separate lines:
 * that is what the transcript already did with a multi-line memory (the text
 * went into HTML, where a newline is whitespace), so folding Markdown in
 * doesn't quietly re-wrap text that has been rendering one way for as long as
 * the panel has existed. Structure comes from the blank lines, lists and
 * headings instead.
 */
export function parseMarkdown(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];

  // The block being accumulated across lines: a paragraph's raw lines, or a
  // list's items. Flushed by a blank line, a different block kind, or EOF.
  let paragraph: string[] = [];
  let list: { ordered: boolean; items: string[] } | null = null;

  const flush = () => {
    if (paragraph.length > 0) {
      blocks.push({ kind: "paragraph", spans: parseInline(paragraph.join(" ")) });
      paragraph = [];
    }
    if (list) {
      blocks.push({ kind: "list", ordered: list.ordered, items: list.items.map(parseInline) });
      list = null;
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    if (FENCE.test(line)) {
      flush();
      const body: string[] = [];
      i++;
      while (i < lines.length && !FENCE.test(lines[i])) body.push(lines[i++]);
      // An unterminated fence (the model ran out of tokens mid-block) takes
      // the rest of the document as code rather than losing it.
      blocks.push({ kind: "codeBlock", text: body.join("\n") });
      continue;
    }

    if (!line.trim()) {
      flush();
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      flush();
      blocks.push({ kind: "heading", level: heading[1].length, spans: parseInline(heading[2]) });
      continue;
    }

    const bullet = BULLET.exec(line);
    const ordered = bullet ? null : ORDERED.exec(line);
    if (bullet || ordered) {
      const isOrdered = ordered !== null;
      // A paragraph directly above a list ends at the list; a list of the
      // other kind starts a new one rather than mixing markers.
      if (paragraph.length > 0 || (list && list.ordered !== isOrdered)) flush();
      if (!list) list = { ordered: isOrdered, items: [] };
      list.items.push((bullet ?? ordered)![1]);
      continue;
    }

    // A plain line directly under a list item is a continuation of the
    // document, not of the item — close the list and start a paragraph.
    if (list) flush();
    paragraph.push(line);
  }

  flush();
  return blocks;
}

/**
 * Collapses a parsed document to a single inline run, for the places that
 * show a memory clamped to a couple of lines (see ChatView's MemoryLine).
 *
 * `-webkit-line-clamp` only clamps inline content, so the collapsed preview
 * can't render block elements — but it can still carry the emphasis, which is
 * the whole point: the preview shows formatted text, not asterisks. Blocks are
 * joined with a space; list items get a bullet so a flattened list still reads
 * as one.
 */
export function flattenBlocks(blocks: Block[]): Inline[] {
  const out: Inline[] = [];
  const push = (spans: Inline[]) => {
    if (out.length > 0 && spans.length > 0) out.push({ kind: "text", text: " " });
    out.push(...spans);
  };

  for (const block of blocks) {
    switch (block.kind) {
      case "paragraph":
      case "heading":
        push(block.spans);
        break;
      case "list":
        for (const item of block.items) push([{ kind: "text", text: "・" }, ...item]);
        break;
      case "codeBlock":
        push([{ kind: "code", text: block.text.replace(/\n/g, " ") }]);
        break;
    }
  }
  return out;
}

/** Parse-and-flatten, the shape a preview actually wants. */
export function parseInlinePreview(src: string): Inline[] {
  return flattenBlocks(parseMarkdown(src));
}
