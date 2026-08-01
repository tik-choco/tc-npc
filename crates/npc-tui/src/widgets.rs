//! Small rendering primitives translated from agent-speech's
//! `view_components.go` (see that file's doc comment for the full
//! vocabulary this is drawn from: `panel` / `fitLines` / `wrapLines` /
//! `chip` / `node` / `edge` / `flowLine` / `statusRow` / etc).
//!
//! Only the parts `ui.rs` actually draws with are kept. agent-speech's
//! `gauge`/`flowLine`/`sectionTitle` were translated first and then removed
//! again: a level meter has no analogue here (there is no audio signal in
//! this client), and the other two turned out to be thin enough that the
//! call sites read better composing `node`/`edge` and `Span` directly.
//!
//! This is a *translation*, not a port. lipgloss (agent-speech's rendering
//! library) works by building a plain `string` for an entire panel — border,
//! padding and all — and handing that string to the terminal; `panel()`,
//! `blockHeight()` and `fitLines()` exist there specifically to make that
//! string come out to an exact width/height *before* it's rendered, because
//! lipgloss itself has no other way to guarantee it. ratatui's model is the
//! opposite: `Block`/`Paragraph` already know their own `Rect` and render
//! directly into it, so a `panel(width, height, lines, color) -> String`
//! equivalent has no job to do here — there is nothing this module could
//! usefully return that `Block::borders(Borders::ALL)` doesn't already give
//! a caller for free. What *doesn't* fall away is everything upstream of
//! that: turning arbitrary (often Japanese) text into the right *number* of
//! *width-bounded* lines before it ever reaches a widget, so a caller can
//! still answer "how tall will this render" ([`block_height`]) without
//! guessing. That half of agent-speech's vocabulary is what this module
//! actually carries over: [`wrap_lines`], [`split_long_lines`],
//! [`crop_lines`], [`truncate_display`], [`pad_display_width`] and
//! [`fit_lines`].
//!
//! # Display width, not byte or char count
//!
//! Every function here that measures or budgets text width does so in
//! *terminal display columns*, using the `unicode-width` crate
//! (`UnicodeWidthStr`/`UnicodeWidthChar`) — never `str::len()` (byte count)
//! and never `.chars().count()` (Unicode scalar count). Both of those are
//! wrong for this UI's actual content: Japanese labels are full of
//! double-width characters (each one occupies *two* terminal columns), so
//! `"ステータス".chars().count()` is 5 but its display width is 10. A
//! truncate or pad written against the wrong number either overflows a
//! bordered panel by a visible margin or leaves ragged, unaligned columns —
//! and unlike an English-only UI, this bug is the *common* case here, not
//! an edge case, because every label in this app is Japanese. Hence the
//! unusually heavy test coverage below on exactly that boundary.
//!
//! `unicode-width` is pinned to the same 0.2 line ratatui itself depends on,
//! and that is not a free choice: measuring widths with a different version
//! than the renderer uses is how off-by-one border corruption gets in. (A
//! 0.1 copy is also in the graph via `unicode-truncate`; that one is
//! ratatui's business, not this module's.)

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme;

/// Display width of `s` in terminal columns (double-width CJK characters
/// count as 2). The single primitive every width-aware function below is
/// built from — see the module doc for why this, and not `len()`/
/// `chars().count()`, is the only correct notion of "width" here.
fn display_width(s: &str) -> u16 {
    // `UnicodeWidthStr::width` returns `usize`; a single terminal line
    // realistically never approaches `u16::MAX` columns, so saturating
    // into `u16` (rather than propagating `usize` through every signature
    // in this module, which mostly deals in `u16` already because that's
    // what ratatui's `Rect` uses) is the pragmatic choice, not a silent
    // truncation risk in practice.
    s.width().min(u16::MAX as usize) as u16
}

/// Display width of a single character, treating anything `unicode-width`
/// doesn't assign a width to (some control characters) as zero rather than
/// panicking or propagating `None` — a terminal line has no meaningful
/// notion of "unknown width", so zero (i.e. "takes no column") is the safe
/// default that keeps every caller's arithmetic total.
fn char_width(c: char) -> u16 {
    c.width().unwrap_or(0) as u16
}

/// Truncate `s` to at most `width` display columns, preferring to cut at a
/// character boundary that leaves room for a trailing `"..."` marker —
/// mirrors agent-speech's `truncate`, which special-cases `width <= 3` to
/// skip the ellipsis entirely (there's no room for both content and marker
/// at that point) and otherwise reserves 3 columns for it.
///
/// # Invariant
/// The returned string's display width never exceeds `width`. This is load-
/// bearing: every layout built on [`fit_lines`] depends on individual lines
/// never overflowing their panel, and this is where that guarantee
/// ultimately comes from. It may render *narrower* than `width` in one
/// unavoidable case: a lone double-width character can't be partially
/// displayed, so if it doesn't fit inside the remaining budget it's simply
/// dropped rather than emitted (see [`split_long_lines`], which shares this
/// rule) — narrower-than-asked is an acceptable outcome, wider-than-asked
/// is not.
pub fn truncate_display(s: &str, width: u16) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(s) <= width {
        return s.to_string();
    }

    if width <= 3 {
        return take_within_width(s, width);
    }

    let mut out = take_within_width(s, width - 3);
    out.push_str("...");
    out
}

/// Greedily accumulate characters from `s` while the running display width
/// stays within `budget`, skipping any single character too wide to fit in
/// whatever budget remains (see [`truncate_display`]'s doc on why that's
/// the only sound option for a lone double-width character).
fn take_within_width(s: &str, budget: u16) -> String {
    let mut out = String::new();
    let mut used = 0u16;
    for c in s.chars() {
        let cw = char_width(c);
        if used + cw > budget {
            // Only bail out entirely once *no* character (not even a
            // narrow one later in the string) could possibly still fit —
            // a wide character here doesn't mean the budget is exhausted,
            // just that this particular character must be skipped.
            if cw > budget {
                continue;
            }
            break;
        }
        out.push(c);
        used += cw;
    }
    out
}

/// Right-pad `s` with spaces until it occupies exactly `width` display
/// columns; returns `s` unchanged if it's already at or past `width` (this
/// deliberately does not truncate — pairing this with [`truncate_display`]
/// first is the caller's job, same division of labor as agent-speech's
/// `padDisplayWidth`). Mirrors that function's use for lining up status
/// labels despite double-width CJK characters, where naive `format!("{:width$}", ...)`
/// padding (which counts `char`s, not display columns) would misalign as
/// soon as any label mixed Japanese and ASCII.
///
/// # Invariant
/// If `display_width(s) <= width`, the result's display width is exactly
/// `width`.
pub fn pad_display_width(s: &str, width: u16) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = s.to_string();
    out.extend(std::iter::repeat(' ').take((width - w) as usize));
    out
}

/// Word-wrap `s` to `width` display columns, splitting on whitespace like
/// agent-speech's `wrapLines` (`strings.Fields`/Rust's `split_whitespace`).
///
/// Word-splitting alone is not enough for this app's content: Japanese
/// sentences routinely contain *no* whitespace at all, so a naive port
/// would treat an entire multi-line paragraph as a single "word" and never
/// wrap it. [`split_long_lines`] is run over the result specifically to
/// catch that — every line this function returns is guaranteed to fit
/// `width`, whitespace or not.
///
/// Returns `vec![String::new()]` for an all-whitespace/empty `s`, matching
/// agent-speech (an empty wrapped block still occupies one blank line, not
/// zero).
pub fn wrap_lines(s: &str, width: u16) -> Vec<String> {
    let width = width.max(1);
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in words {
        if current.is_empty() {
            current = word.to_string();
            continue;
        }
        let candidate_width = display_width(&current) + 1 + display_width(word);
        if candidate_width <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    split_long_lines(&lines, width)
}

/// Hard-break any line in `lines` wider than `width`, character by
/// character — the fallback [`wrap_lines`] relies on for whitespace-free
/// (e.g. Japanese) text, and usable standalone when input is already
/// "words" in some other sense. Mirrors agent-speech's `splitLongLines`.
///
/// # Invariant
/// Every returned line's display width is at most `width`. Unlike the
/// agent-speech original (whose Go loop admits a single over-wide rune as
/// its own too-wide line when the accumulator was empty), this deliberately
/// drops a lone character that cannot fit even a fresh, empty line, rather
/// than emit a line that violates the width bound — every consumer of this
/// module needs "never exceeds width" to be an actual guarantee, not "true
/// except for one input shape".
pub fn split_long_lines(lines: &[String], width: u16) -> Vec<String> {
    let width = width.max(0);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if display_width(line) <= width {
            out.push(line.clone());
            continue;
        }

        let mut current = String::new();
        let mut current_w = 0u16;
        for c in line.chars() {
            let cw = char_width(c);
            if cw > width {
                // See the doc comment: a character wider than the entire
                // budget can never legally appear in any line, so it's
                // dropped rather than smuggled in on a line of its own.
                continue;
            }
            if current_w + cw > width {
                out.push(std::mem::take(&mut current));
                current_w = 0;
            }
            current.push(c);
            current_w += cw;
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

/// Keep only the last `height` entries of `lines` — i.e. crop from the
/// *top*, not the bottom, so a scrolling chat/log panel shows its most
/// recent content rather than its oldest. Mirrors agent-speech's
/// `cropLines`.
///
/// # Invariant
/// The result never has more than `height` entries (zero if `height == 0`).
pub fn crop_lines(lines: &[String], height: u16) -> Vec<String> {
    let height = height as usize;
    if height == 0 {
        return Vec::new();
    }
    if lines.len() <= height {
        return lines.to_vec();
    }
    lines[lines.len() - height..].to_vec()
}

/// Combine [`crop_lines`] + [`truncate_display`] + bottom-padding into
/// exactly `height` ready-to-render [`Line`]s, each at most `width` display
/// columns wide — the one-call building block for "I have some text, I have
/// a fixed-size box, make it fit exactly". Mirrors agent-speech's
/// `fitLines`, with one difference: that version also pads each line's
/// *content* out to `width` with trailing spaces, because lipgloss needs an
/// explicit-width string to render a correctly-sized box. ratatui doesn't:
/// a `Paragraph` already fills its `Rect` regardless of how short its lines
/// are, so padding content here would be dead work — only the *line count*
/// needs padding (with empty lines) to guarantee the height.
///
/// # Invariant
/// Returns exactly `height` lines, each with display width at most `width`.
/// This is the pairing [`block_height`] is meant to be checked against:
/// `fit_lines(..., w, h).len() == h` always.
pub fn fit_lines(lines: &[String], width: u16, height: u16) -> Vec<Line<'static>> {
    let cropped = crop_lines(lines, height);
    let mut out: Vec<Line<'static>> = cropped
        .iter()
        .map(|l| Line::from(truncate_display(l, width)))
        .collect();
    while (out.len() as u16) < height {
        out.push(Line::from(""));
    }
    out
}

/// Number of terminal rows a already-rendered block occupies — one row per
/// [`Line`], since every function in this module hands back text that has
/// already been wrapped/cropped to its final line count (as opposed to raw
/// text later handed to a wrapping `Paragraph`, which would pick its own
/// row count at render time). Mirrors agent-speech's `blockHeight`, which
/// counts newlines in an already-rendered lipgloss string for the same
/// "how much vertical budget did this actually use" purpose.
///
/// # Invariant
/// Equals `rendered.len()` exactly — the whole point of this helper is to
/// be trivially true so a layout can subtract it from a remaining-height
/// budget without re-deriving how tall something rendered.
pub fn block_height(rendered: &[Line<'_>]) -> u16 {
    rendered.len().min(u16::MAX as usize) as u16
}

/// A small coloured badge — a status pill like `" 接続中 "`. Mirrors
/// agent-speech's `chip`, including its one column of padding on each side
/// (`Padding(0, 1)` in lipgloss) — ratatui `Style` has no padding concept of
/// its own for a `Span`, so the padding is literal leading/trailing spaces
/// baked into the rendered text here instead.
///
/// Returns a [`Line`] rather than a bare [`Span`] so it can stand alone as a
/// row; `ui.rs`'s top bar takes `.spans` off it to sit several chips inline.
pub fn chip(label: &str, color: ratatui::style::Color) -> Line<'static> {
    Line::from(Span::styled(format!(" {label} "), theme::chip_style(color)))
}

/// A single node in a flow diagram (e.g. `センサー -> 記憶 -> 発話` pipeline
/// stages), colored by whether it's the currently-active stage. Mirrors
/// agent-speech's `node(label, active)`; same one-space padding as [`chip`]
/// for the same reason.
///
/// Returns a bare [`Span`] (unlike [`chip`]) because nodes are meant to be
/// assembled inline with [`edge`] spans into a single [`Line`] via
/// [`flow_line`] — that's the whole point of the flow-diagram vocabulary.
pub fn node(label: &str, active: bool) -> Span<'static> {
    Span::styled(format!(" {label} "), theme::node_style(active))
}

/// The muted arrow between two [`node`]s in a flow diagram, e.g.
/// `--送信-->`. Mirrors agent-speech's `edge(label)`.
///
/// An empty label gives a plain `-->` rather than agent-speech's literal
/// `---->`: a pipeline whose stages need no annotation is the common case
/// here (`talk → memory → speech …` has nothing to say between stages), and
/// interpolating an empty label into the decorated form rendered as visual
/// noise on screen — which is how this was caught.
pub fn edge(label: &str) -> Span<'static> {
    let text = if label.is_empty() {
        "-->".to_string()
    } else {
        format!("--{label}-->")
    };
    Span::styled(text, theme::muted())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- display width plumbing ---------------------------------------

    #[test]
    fn ascii_width_is_char_count() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn fullwidth_japanese_is_double_width() {
        // 5 characters, each double-width -> 10 columns. This is the exact
        // miscount `.chars().count()` would produce (5) if this module
        // used it instead of unicode-width.
        assert_eq!(display_width("ステータス"), 10);
    }

    #[test]
    fn mixed_ascii_and_japanese_width() {
        // "v1" (2 narrow) + " " (1 narrow) + "キャラ" (3 wide chars * 2) = 9.
        assert_eq!(display_width("v1 キャラ"), 9);
    }

    // --- truncate_display -----------------------------------------------

    #[test]
    fn truncate_noop_when_already_short_enough() {
        assert_eq!(truncate_display("hi", 10), "hi");
        assert_eq!(truncate_display("ステータス", 10), "ステータス");
    }

    #[test]
    fn truncate_never_exceeds_width_ascii() {
        for width in 0..12u16 {
            let out = truncate_display("hello world, this is long", width);
            assert!(
                display_width(&out) <= width,
                "width={width} out={out:?} out_width={}",
                display_width(&out)
            );
        }
    }

    #[test]
    fn truncate_never_exceeds_width_on_fullwidth_boundary() {
        // Every width from 0 up through well past the full string, over a
        // string made entirely of double-width characters, so any
        // off-by-one in the "cut mid-character" handling shows up as an
        // overflow somewhere in this sweep.
        let s = "日本語のテキストをたくさん入れる";
        for width in 0..40u16 {
            let out = truncate_display(s, width);
            assert!(
                display_width(&out) <= width,
                "width={width} out={out:?} out_width={}",
                display_width(&out)
            );
        }
    }

    #[test]
    fn truncate_uses_ellipsis_when_room_allows() {
        let out = truncate_display("abcdefghij", 6);
        assert_eq!(out, "abc...");
        assert!(display_width(&out) <= 6);
    }

    #[test]
    fn truncate_skips_ellipsis_at_width_three_or_less() {
        // Go's `truncate` special-cases width<=3 to skip the ellipsis
        // entirely (no room for content + marker); this test pins that the
        // Rust port does the same rather than always trying to reserve 3
        // columns regardless of how small width is.
        let out = truncate_display("abcdef", 3);
        assert_eq!(out, "abc");
        assert!(!out.contains("..."));
    }

    #[test]
    fn truncate_maximizes_within_bound_not_just_avoids_overflow() {
        // "Never exceed" alone would be satisfied by returning "" for
        // every input; this pins that truncate also uses as much of the
        // budget as it legally can.
        let out = truncate_display("abcdefgh", 5);
        assert_eq!(display_width(&out), 5);
    }

    #[test]
    fn truncate_of_pure_fullwidth_text_uses_ellipsis_correctly() {
        let out = truncate_display("あいうえおかきく", 6);
        // width budget for content = 6-3 = 3 columns = 1 full-width char.
        assert_eq!(out, "あ...");
        assert!(display_width(&out) <= 6);
    }

    // --- pad_display_width ------------------------------------------------

    #[test]
    fn pad_aligns_mixed_width_labels() {
        let a = pad_display_width("talk", 10);
        let b = pad_display_width("キャラ", 10);
        assert_eq!(display_width(&a), 10);
        assert_eq!(display_width(&b), 10);
    }

    #[test]
    fn pad_is_noop_when_already_at_or_over_width() {
        assert_eq!(pad_display_width("ステータス", 4), "ステータス");
        assert_eq!(pad_display_width("hello", 5), "hello");
    }

    // --- wrap_lines / split_long_lines -------------------------------------

    #[test]
    fn wrap_lines_never_exceeds_width_with_ascii_words() {
        let s = "the quick brown fox jumps over the lazy dog again and again";
        for width in [1u16, 3, 8, 20] {
            for line in wrap_lines(s, width) {
                assert!(display_width(&line) <= width, "width={width} line={line:?}");
            }
        }
    }

    #[test]
    fn wrap_lines_handles_whitespace_free_japanese_text() {
        // No spaces at all -- this is the case a literal port of
        // strings.Fields-based wrapping would fail on by returning the
        // entire sentence as a single overlong "word".
        let s = "これはとても長い日本語の文章でスペースが一切含まれていません";
        for width in [1u16, 2, 5, 12] {
            for line in wrap_lines(s, width) {
                assert!(display_width(&line) <= width, "width={width} line={line:?}");
            }
        }
    }

    #[test]
    fn wrap_lines_mixed_ascii_and_fullwidth_never_exceeds_width() {
        let s = "status: 接続中です メモリ使用量が高くなっています warning";
        for width in [4u16, 7, 15] {
            for line in wrap_lines(s, width) {
                assert!(display_width(&line) <= width, "width={width} line={line:?}");
            }
        }
    }

    #[test]
    fn wrap_lines_empty_input_yields_one_blank_line() {
        assert_eq!(wrap_lines("", 10), vec![String::new()]);
        assert_eq!(wrap_lines("   ", 10), vec![String::new()]);
    }

    #[test]
    fn split_long_lines_never_exceeds_width() {
        let lines = vec!["a".repeat(50), "日本語".repeat(20)];
        for width in [0u16, 1, 2, 3, 9] {
            for line in split_long_lines(&lines, width) {
                assert!(display_width(&line) <= width, "width={width} line={line:?}");
            }
        }
    }

    // --- crop_lines / fit_lines --------------------------------------------

    #[test]
    fn crop_lines_keeps_the_most_recent_entries() {
        let lines: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let cropped = crop_lines(&lines, 3);
        assert_eq!(cropped, vec!["7", "8", "9"]);
    }

    #[test]
    fn crop_lines_never_exceeds_requested_height() {
        let lines: Vec<String> = (0..5).map(|i| i.to_string()).collect();
        for height in 0..8u16 {
            assert!(crop_lines(&lines, height).len() as u16 <= height);
        }
    }

    #[test]
    fn crop_lines_zero_height_is_empty_not_panicking() {
        let lines = vec!["x".to_string()];
        assert!(crop_lines(&lines, 0).is_empty());
    }

    #[test]
    fn fit_lines_always_returns_exactly_height_lines() {
        let lines: Vec<String> = vec!["short".into(), "a much longer line of text here".into()];
        for height in 0..6u16 {
            for width in [0u16, 1, 5, 20] {
                let out = fit_lines(&lines, width, height);
                assert_eq!(out.len() as u16, height, "width={width} height={height}");
            }
        }
    }

    #[test]
    fn fit_lines_never_exceeds_width() {
        let lines: Vec<String> = vec!["日本語の長い一行テキストです".into(), "short".into()];
        for width in [0u16, 1, 3, 7, 15] {
            let out = fit_lines(&lines, width, 4);
            for line in &out {
                let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(
                    display_width(&rendered) <= width,
                    "width={width} rendered={rendered:?}"
                );
            }
        }
    }

    #[test]
    fn fit_lines_pads_short_input_with_blank_lines() {
        let lines: Vec<String> = vec!["one".into()];
        let out = fit_lines(&lines, 10, 4);
        assert_eq!(out.len(), 4);
        // First line carries content, the rest are the padding blanks.
        let first: String = out[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(first, "one");
    }

    // --- block_height --------------------------------------------------

    #[test]
    fn block_height_matches_line_count() {
        let lines = vec![Line::from("a"), Line::from("b"), Line::from("c")];
        assert_eq!(block_height(&lines), 3);
    }

    #[test]
    fn block_height_of_fit_lines_output_matches_requested_height() {
        let content: Vec<String> = vec!["hello".into()];
        for height in 0..5u16 {
            let rendered = fit_lines(&content, 10, height);
            assert_eq!(block_height(&rendered), height);
        }
    }

    // --- chip / node / gauge: no panics at degenerate widths -----------

    #[test]
    fn chip_does_not_panic_on_empty_label() {
        let _ = chip("", theme::PRIMARY);
    }

    #[test]
    fn node_does_not_panic_active_and_inactive() {
        let _ = node("状態", true);
        let _ = node("状態", false);
    }

    #[test]
    fn edge_renders_arrow_around_label() {
        let span = edge("送信");
        assert_eq!(span.content.as_ref(), "--送信-->");
    }
}
