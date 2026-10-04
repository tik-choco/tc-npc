// Renders the Markdown subset parsed by lib/markdown.ts.
//
// Everything here builds VNodes — there is no `dangerouslySetInnerHTML`
// anywhere, so the tree can only ever contain the handful of elements below
// no matter what the model wrote. That is the point: the text this renders is
// LLM-authored (memory documents, see ChatView's MemoryLine), which is
// precisely the input one does not want to hand to an HTML pipeline.
//
// Two entry points, because the caller's layout decides which is usable:
// - <Markdown> emits blocks (paragraphs, lists, headings, code) and needs a
//   block container.
// - <MarkdownInline> flattens the same document to a single inline run, for
//   somewhere that can only hold inline content — notably a
//   `-webkit-line-clamp` preview, which only clamps inline boxes.
import type { ComponentChildren } from "preact";

import { parseInlinePreview, parseMarkdown, type Inline } from "../lib/markdown";
import "../styles/components.css";

/** Keys are positional, which is safe here: a render is a pure function of
 *  `text`, so a given position always holds the same node kind unless the
 *  text itself changed — in which case re-creating the node is correct. */
function renderInline(nodes: Inline[]): ComponentChildren {
  return nodes.map((node, i) => {
    switch (node.kind) {
      case "text":
        return node.text;
      case "strong":
        return <strong key={i}>{renderInline(node.children)}</strong>;
      case "em":
        return <em key={i}>{renderInline(node.children)}</em>;
      case "code":
        return (
          <code key={i} class="md-code">
            {node.text}
          </code>
        );
      case "link":
        // `noreferrer` covers `noopener` in every browser this console runs
        // in, but both are spelled out: the pair is the thing being asserted,
        // and a link in model-written text is not a link anyone vetted.
        return (
          <a key={i} href={node.href} target="_blank" rel="noreferrer noopener">
            {renderInline(node.children)}
          </a>
        );
    }
  });
}

export interface MarkdownProps {
  text: string;
  /** Extra class on the block container, for callers that need to scope
   *  their own type sizing to it. */
  class?: string;
}

/** Block-level render: paragraphs, lists, headings and fenced code. */
export function Markdown({ text, class: className }: MarkdownProps) {
  const blocks = parseMarkdown(text);
  return (
    <div class={className ? `md ${className}` : "md"}>
      {blocks.map((block, i) => {
        switch (block.kind) {
          case "paragraph":
            return <p key={i}>{renderInline(block.spans)}</p>;
          case "heading": {
            // Headings inside a memory document are structure *within* a
            // transcript row, not page structure, so they render as one
            // styled element with the level as data rather than as h1..h6
            // competing with the app's real heading outline.
            return (
              <p key={i} class="md-heading" data-level={block.level}>
                {renderInline(block.spans)}
              </p>
            );
          }
          case "list":
            return block.ordered ? (
              <ol key={i}>
                {block.items.map((item, j) => (
                  <li key={j}>{renderInline(item)}</li>
                ))}
              </ol>
            ) : (
              <ul key={i}>
                {block.items.map((item, j) => (
                  <li key={j}>{renderInline(item)}</li>
                ))}
              </ul>
            );
          case "codeBlock":
            return (
              <pre key={i} class="md-pre">
                <code>{block.text}</code>
              </pre>
            );
        }
      })}
    </div>
  );
}

/** Single-run render for inline-only contexts — see the header. */
export function MarkdownInline({ text }: { text: string }) {
  return <>{renderInline(parseInlinePreview(text))}</>;
}
