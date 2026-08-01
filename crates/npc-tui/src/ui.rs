//! Rendering only — every value this module reads comes straight off [`App`]
//! and nothing here mutates it. Kept separate from `app.rs` so the state
//! transitions there stay testable without a terminal (see that module's
//! doc comment), and separate from `lib.rs` so the event loop isn't tangled
//! up with layout code.
//!
//! ## Why the layout is measured rather than declared
//!
//! The obvious ratatui shape — one `Layout` of fixed `Constraint::Length`s —
//! breaks as soon as the terminal is shorter than their sum: sections
//! collapse to zero height in whatever order the solver picks, and the piece
//! that vanishes is rarely the one you would have chosen to lose. The first
//! version of this file did exactly that, with `Length(1)/Length(1)/Min(3)/
//! Length(3)/Length(1)`, and a 6-row terminal left nothing legible.
//!
//! So the fixed furniture (top bar, module flow, error panel, input, footer)
//! is built as lines *first* and measured with [`block_height`]; whatever is
//! left over is divided among the panels that can absorb it. A layout that
//! knows what each piece actually cost can degrade on purpose.
//!
//! Below [`COMPACT_COLS`]×[`COMPACT_ROWS`] the main view drops pieces in a
//! fixed priority order: event log first (a nicety), then the module flow
//! (redundant with the top bar's status chip), then the log panel. The top
//! bar, input line and footer are never dropped — without them you cannot
//! tell what you are attached to, type, or find out how to quit.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ChatRow, ConfigFocus, Focus, MainFocus, SystemActivity, ViewState};
use crate::theme;
use crate::widgets::{self, block_height, chip, fit_lines, truncate_display, wrap_lines};

/// Terminals smaller than this get the compact main view. 80×24 is the
/// classic default and comfortably fits the full layout; compact exists for
/// split panes and phone-sized SSH clients, not for the normal case.
const COMPACT_COLS: u16 = 80;
const COMPACT_ROWS: u16 = 24;

/// A bordered panel costs two rows before any content shows, so below this
/// it is dropped rather than drawn as a box with nothing inside it.
const MIN_PANEL_HEIGHT: u16 = 3;

/// The event log is the least important panel on screen; bounding it stops it
/// from stealing transcript rows on a tall display.
const EVENT_LOG_MIN: u16 = 4;
const EVENT_LOG_MAX: u16 = 6;

/// Chat/log split when they sit side by side. The transcript is what you
/// opened this for; the log is context.
const CHAT_PERCENT: u16 = 62;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    // Terminals really do report 0×0 mid-resize. Nothing sensible to draw,
    // but it must not take the process down either.
    if area.width == 0 || area.height == 0 {
        return;
    }

    match app.view {
        ViewState::Main => draw_main(frame, area, app),
        ViewState::Config => draw_config(frame, area, app),
    }
}

// ---------------------------------------------------------------------------
// Main view
// ---------------------------------------------------------------------------

fn draw_main(frame: &mut Frame, area: Rect, app: &App) {
    let compact = area.width < COMPACT_COLS || area.height < COMPACT_ROWS;

    // Pass one: build the furniture, then ask what it cost.
    let top = top_bar(app, area.width);
    let input = input_line(app, area.width);
    let footer = footer_line(compact);
    let error = error_lines(app, area.width, if compact { 2 } else { 4 });
    let flow = if compact {
        Vec::new()
    } else {
        flow_row(app, area.width)
    };

    let fixed = block_height(&top)
        + block_height(&input)
        + block_height(&footer)
        + block_height(&error)
        + block_height(&flow);

    // Pass two: everything else is the panels' budget. `saturating_sub`
    // carries real weight here — on a 3-row terminal the furniture alone
    // exceeds the height, and an underflow would panic rather than degrade.
    let remaining = area.height.saturating_sub(fixed);

    let event_height = if !compact && remaining >= 14 {
        (remaining / 6).clamp(EVENT_LOG_MIN, EVENT_LOG_MAX)
    } else {
        0
    };
    let panels_height = remaining.saturating_sub(event_height);

    let mut constraints = vec![Constraint::Length(block_height(&top))];
    if !flow.is_empty() {
        constraints.push(Constraint::Length(block_height(&flow)));
    }
    if !error.is_empty() {
        constraints.push(Constraint::Length(block_height(&error)));
    }
    constraints.push(Constraint::Length(panels_height));
    if event_height > 0 {
        constraints.push(Constraint::Length(event_height));
    }
    constraints.push(Constraint::Length(block_height(&input)));
    constraints.push(Constraint::Length(block_height(&footer)));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut i = 0;
    frame.render_widget(Paragraph::new(top), rows[i]);
    i += 1;
    if !flow.is_empty() {
        frame.render_widget(Paragraph::new(flow), rows[i]);
        i += 1;
    }
    if !error.is_empty() {
        frame.render_widget(Paragraph::new(error), rows[i]);
        i += 1;
    }
    if panels_height > 0 {
        draw_panels(frame, rows[i], app, compact);
    }
    i += 1;
    if event_height > 0 {
        draw_event_log(frame, rows[i], app);
        i += 1;
    }
    frame.render_widget(Paragraph::new(input), rows[i]);
    i += 1;
    frame.render_widget(Paragraph::new(footer), rows[i]);
}

/// Chat and log: side by side when wide enough for both to hold readable
/// text, stacked when tall but narrow, chat alone when only one fits — a log
/// with no transcript above it is the wrong one to keep.
fn draw_panels(frame: &mut Frame, area: Rect, app: &App, compact: bool) {
    if area.height < MIN_PANEL_HEIGHT {
        return;
    }

    if !compact && area.width >= COMPACT_COLS {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(CHAT_PERCENT),
                Constraint::Percentage(100 - CHAT_PERCENT),
            ])
            .split(area);
        draw_chat(frame, cols[0], app);
        draw_log(frame, cols[1], app);
    } else if area.height >= MIN_PANEL_HEIGHT * 2 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area);
        draw_chat(frame, rows[0], app);
        draw_log(frame, rows[1], app);
    } else {
        draw_chat(frame, area, app);
    }
}

fn draw_chat(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("チャット")
        .borders(Borders::ALL)
        .border_style(theme::border(app.focus == Focus::Main(MainFocus::Chat)));

    let mut lines: Vec<Line> = Vec::with_capacity(app.chat.len() + 1);
    for row in &app.chat {
        lines.push(match row {
            ChatRow::Chat { role, text } if role == "assistant" => Line::from(vec![
                Span::styled("NPC: ", theme::title(theme::PRIMARY)),
                Span::raw(text.clone()),
            ]),
            ChatRow::Chat { text, .. } => Line::from(vec![
                Span::styled("You: ", theme::title(theme::SECONDARY)),
                Span::raw(text.clone()),
            ]),
            // Rendered, not skipped: a turn the NPC deliberately sat out is
            // information, and a gap would read as a dropped message.
            ChatRow::Silent { reason } => Line::from(Span::styled(
                format!("(反応なし: {reason})"),
                theme::muted().add_modifier(Modifier::ITALIC),
            )),
        });
    }
    if app.pending {
        lines.push(Line::from(Span::styled("NPC: ...", theme::muted())));
    }

    render_scrollable(frame, area, block, lines, app.chat_scroll);
}

fn draw_log(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("ログ")
        .borders(Borders::ALL)
        .border_style(theme::border(app.focus == Focus::Main(MainFocus::Log)));

    let lines: Vec<Line> = app
        .log
        .iter()
        .map(|row| {
            Line::from(vec![
                Span::styled(format!("[{}] ", row.label), Style::new().fg(theme::WARNING)),
                Span::raw(row.text.clone()),
            ])
        })
        .collect();

    render_scrollable(frame, area, block, lines, app.log_scroll);
}

/// Operations and state changes — toggles pressed, reconnects — as opposed to
/// the conversation (chat panel) or what the NPC sensed (log panel). Always
/// pinned to the newest entry; there is no scroll state for it, because
/// anything worth going back for is in the log panel too.
fn draw_event_log(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("イベント")
        .borders(Borders::ALL)
        .border_style(theme::border(false));
    let lines: Vec<Line> = app
        .event_log
        .iter()
        .map(|entry| Line::from(Span::styled(entry.clone(), theme::muted())))
        .collect();
    render_scrollable(frame, area, block, lines, 0);
}

/// Shared rendering for the scrollable panels: word-wraps `lines` inside
/// `block`, then bottom-anchors the viewport and scrolls it up by `scroll_up`
/// lines from there.
///
/// `Paragraph::line_count` (added upstream specifically so callers can build
/// scrollbar-style logic on top of it) is what makes an exact bottom-anchor
/// possible without hand-rolling unicode-width-aware wrapping here: it
/// reports how many lines `text` reflows to at a given width using the exact
/// same wrapping `Wrap { trim: false }` will use to render it, so the count
/// computed here and the layout actually drawn can never disagree.
fn render_scrollable(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    lines: Vec<Line>,
    scroll_up: u16,
) {
    let inner = block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        // Still draw the border: a panel that vanishes entirely reads as a
        // layout bug, whereas an empty box reads as "no room for content".
        frame.render_widget(block, area);
        return;
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });

    let total_lines = paragraph.line_count(inner.width) as u16;
    let max_scroll = total_lines.saturating_sub(inner.height);
    // `scroll_up` is "how many lines above the auto-follow position", not an
    // absolute index — see `App::chat_scroll`/`log_scroll` — so a value past
    // the top (after a fast PageUp saturates it) is clamped here rather than
    // treated as an error; nothing needs feeding back into `App`.
    let scroll = max_scroll.saturating_sub(scroll_up.min(max_scroll));

    frame.render_widget(paragraph.block(block).scroll((scroll, 0)), area);
}

// ---------------------------------------------------------------------------
// Fixed furniture
// ---------------------------------------------------------------------------

fn top_bar(app: &App, width: u16) -> Vec<Line<'static>> {
    let activity = app.system_activity();
    let mut spans = vec![
        Span::styled("tc-npc", theme::title(theme::HIGHLIGHT)),
        Span::raw(" "),
    ];
    spans.extend(chip(activity.label(), activity_color(activity)).spans);
    spans.push(Span::raw(" "));

    let character = match &app.character {
        Some(c) => format!("キャラ: {}", c.name),
        None => "キャラ: (未設定)".to_string(),
    };
    spans.extend(chip(&character, theme::MUTED).spans);

    // The notice rides on the top bar rather than owning a row: it is one
    // transient line, and a permanent row for it would cost a line of
    // transcript on every frame with nothing to say.
    if let Some(notice) = &app.notice {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            truncate_display(notice, width / 3),
            theme::muted(),
        ));
    }

    vec![Line::from(spans)]
}

/// The module pipeline: which parts of the process are live right now, at a
/// glance. This is the one thing the top bar's single status chip cannot
/// convey, and it is why `agent-speech` draws its STT/TTS flow the same way.
fn flow_row(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (name, on)) in app.module_states().iter().enumerate() {
        if i > 0 {
            spans.push(widgets::edge(""));
        }
        spans.push(widgets::node(name, *on));
    }
    let line = Line::from(spans);
    // One row, and only if it fits: a wrapped pipeline diagram is worse than
    // no pipeline diagram.
    if line.width() as u16 > width {
        return Vec::new();
    }
    vec![line]
}

/// The error panel, capped. An unbounded wrapped error on a short terminal
/// would push the transcript off screen — the failure report swallowing the
/// thing you were trying to read.
fn error_lines(app: &App, width: u16, cap: u16) -> Vec<Line<'static>> {
    if let Some((message, _)) = &app.error {
        let wrapped = wrap_lines(&format!("エラー: {message}"), width);
        let mut lines = fit_lines(&wrapped, width, cap);
        for line in lines.iter_mut() {
            for span in line.spans.iter_mut() {
                span.style = Style::new().fg(theme::ERROR);
            }
        }
        return lines;
    }
    // `status_line` is the protocol-level fallback (a frame that failed to
    // parse, say). No TTL and lower priority, so it only gets the space when
    // no real error is competing for it.
    if let Some(status) = &app.status_line {
        let wrapped = wrap_lines(status, width);
        return fit_lines(&wrapped, width, cap);
    }
    Vec::new()
}

fn input_line(app: &App, width: u16) -> Vec<Line<'static>> {
    let focused = app.focus == Focus::Main(MainFocus::Input);
    // A trailing glyph rather than a real terminal cursor: this crate never
    // calls `Terminal::set_cursor`, and a text caret is what the contract
    // tests can see, so what they check and what the user sees stay the same
    // thing.
    let text = if focused {
        format!("> {}_", app.input)
    } else {
        format!("  {}", app.input)
    };
    vec![Line::from(Span::styled(
        truncate_display(&text, width),
        if focused {
            theme::title(theme::TEXT)
        } else {
            theme::muted()
        },
    ))]
}

fn footer_line(compact: bool) -> Vec<Line<'static>> {
    let hints = if compact {
        "Tab:移動  F2:設定  Ctrl+C:終了"
    } else {
        "Tab:フォーカス  F2:設定  Enter:送信  PgUp/PgDn:スクロール  Ctrl+C:終了"
    };
    vec![Line::from(Span::styled(hints, theme::muted()))]
}

// ---------------------------------------------------------------------------
// Config view
// ---------------------------------------------------------------------------

/// Modules whose on/off is only read once, when `main.rs` decides what to
/// spawn — changing them takes a restart. The rest either idle in place and
/// watch the `npc:config` bus topic, or are always spawned anyway. Saying so
/// on screen is the whole reason this list is here: a toggle that silently
/// does nothing until the next launch is worse than no toggle.
const RESTART_REQUIRED: &[&str] = &["talk", "memory", "vision", "action"];

fn draw_config(frame: &mut Frame, area: Rect, app: &App) {
    let top = top_bar(app, area.width);
    let footer = vec![Line::from(Span::styled(
        "Tab/Esc:監視画面へ戻る  Ctrl+C:終了",
        theme::muted(),
    ))];
    let fixed = block_height(&top) + block_height(&footer);
    let body_height = area.height.saturating_sub(fixed);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(block_height(&top)),
            Constraint::Length(body_height),
            Constraint::Length(block_height(&footer)),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(top), rows[0]);

    if body_height > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[1]);
        draw_config_modules(frame, cols[0], app);
        draw_config_characters(frame, cols[1], app);
    }

    frame.render_widget(Paragraph::new(footer), rows[2]);
}

/// Read-only, and labelled as such. These flags are consulted once, when
/// `main.rs` decides what to spawn, so a checkbox here would appear to work
/// and then do nothing until the next launch.
fn draw_config_modules(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("モジュール (表示のみ)")
        .borders(Borders::ALL)
        .border_style(theme::border(
            app.focus == Focus::Config(ConfigFocus::Modules),
        ));

    // Widest module name, so the "要再起動" column lines up instead of
    // ragging along the right of names of different lengths.
    let name_col = app
        .module_states()
        .iter()
        .map(|(name, _)| name.len() as u16)
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line> = Vec::new();
    for (name, on) in app.module_states() {
        let mut spans = vec![
            Span::styled(
                if on { "[x] " } else { "[ ] " }.to_string(),
                theme::node_style(on),
            ),
            Span::raw(widgets::pad_display_width(name, name_col)),
        ];
        if RESTART_REQUIRED.contains(&name) {
            spans.push(Span::styled("  要再起動", theme::muted()));
        }
        lines.push(Line::from(spans));
    }
    render_scrollable(frame, area, block, lines, 0);
}

/// The one thing this screen can actually change. Switching the active
/// character takes effect immediately server-side (unlike the module flags
/// opposite), which is exactly why it is the write action offered here.
fn draw_config_characters(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Config(ConfigFocus::Characters);
    let block = Block::default()
        .title("キャラクタ (Enter: 切替)")
        .borders(Borders::ALL)
        .border_style(theme::border(focused));

    if app.characters.is_empty() {
        let lines = vec![Line::from(Span::styled("(取得中/なし)", theme::muted()))];
        render_scrollable(frame, area, block, lines, 0);
        return;
    }

    let lines: Vec<Line> = app
        .characters
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let cursor = if focused && i == app.character_cursor {
                "> "
            } else {
                "  "
            };
            let mark = if entry.active { "●" } else { "○" };
            Line::from(vec![
                Span::styled(format!("{cursor}{mark} "), theme::node_style(entry.active)),
                Span::raw(entry.name.clone()),
            ])
        })
        .collect();
    render_scrollable(frame, area, block, lines, 0);
}

// ---------------------------------------------------------------------------

fn activity_color(activity: SystemActivity) -> Color {
    match activity {
        SystemActivity::Offline => theme::ERROR,
        SystemActivity::Connecting => theme::WARNING,
        SystemActivity::Waiting => theme::SECONDARY,
        SystemActivity::Idle => theme::SUCCESS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ConnectionState, LogRow};
    use crate::protocol::CharacterRef;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The sizes the contract is checked against. The extremes are the point:
    /// 1×1 and 20×5 are where height arithmetic underflows and zero-size
    /// rects appear, and those are panics, not cosmetic defects.
    const SIZES: &[(u16, u16)] = &[
        (1, 1),
        (4, 2),
        (10, 3),
        (20, 5),
        (40, 12),
        (79, 23),
        (80, 24),
        (120, 40),
        (200, 60),
    ];

    fn populated() -> App {
        let mut app = App::default();
        app.set_connection(ConnectionState::Connected);
        app.character = Some(CharacterRef {
            id: "c1".into(),
            name: "ちょこ".into(),
        });
        app.modules.talk = true;
        app.modules.speech = true;
        app.modules.scheduler = true;
        for i in 0..40 {
            app.chat.push(ChatRow::Chat {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                text: format!("これは{i}番目のテスト発話です。全角文字を含みます。"),
            });
            // Deliberately share no substring with the event log's entries or
            // the "イベント" panel title: the first version of this fixture
            // said "感覚イベント" here, which made every `contains("イベント")`
            // assertion pass whether the event panel was on screen or not.
            app.log.push(LogRow {
                label: "sense".into(),
                text: format!("感覚ログ {i}"),
            });
            app.add_event_log(format!("操作 {i}"));
        }
        app
    }

    fn states() -> Vec<(&'static str, App)> {
        let mut long_error = populated();
        long_error.set_error(
            "非常に長いエラーメッセージです。".repeat(20),
            std::time::Instant::now(),
        );

        let mut with_notice = populated();
        with_notice.set_notice("設定を保存しました");

        let mut config = populated();
        config.toggle_view();

        vec![
            ("empty", App::default()),
            ("populated", populated()),
            ("long_error", long_error),
            ("with_notice", with_notice),
            ("config", config),
        ]
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.area.width, w, "buffer width drifted");
        assert_eq!(buffer.area.height, h, "buffer height drifted");
        // A double-width character occupies two cells: the first carries the
        // symbol, the second is a filler space. Concatenating every cell
        // naively therefore turns "チャット" into "チ ャ ッ ト" and every
        // `contains` assertion on Japanese text silently fails — which is
        // exactly what happened the first time this helper was written.
        // Advance by the symbol's display width instead.
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                out.push_str(symbol);
                x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            out.push('\n');
        }
        out
    }

    /// The contract: every state renders at every size without panicking, and
    /// the buffer keeps the frame's exact dimensions. The absence of a panic
    /// is what carries weight — that is precisely the failure mode a layout
    /// of fixed `Length`s produces on a small terminal, and the reason this
    /// file measures before it allocates.
    #[test]
    fn renders_at_every_size_without_panicking() {
        for (w, h) in SIZES {
            for (name, app) in states() {
                let out =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&app, *w, *h)));
                assert!(out.is_ok(), "state {name} panicked at {w}x{h}");
            }
        }
    }

    /// The pieces that must survive however small it gets: without the title
    /// you cannot tell what you are attached to, and without the footer you
    /// cannot find out how to leave.
    #[test]
    fn top_bar_and_footer_survive_a_small_terminal() {
        let rendered = render(&populated(), 40, 12);
        assert!(rendered.contains("tc-npc"), "top bar missing:\n{rendered}");
        assert!(rendered.contains("Ctrl+C"), "footer missing:\n{rendered}");
    }

    /// The other direction: the pieces compact mode drops must actually be
    /// present at full size, or the degradation path would be untested where
    /// it matters.
    #[test]
    fn full_size_shows_what_compact_mode_drops() {
        let rendered = render(&populated(), 120, 40);
        assert!(rendered.contains("チャット"), "chat panel missing");
        assert!(rendered.contains("ログ"), "log panel missing");
        assert!(rendered.contains("イベント"), "event log missing");
        assert!(rendered.contains("talk"), "module flow missing");
    }

    /// Compact really is compact: the event log is the first thing dropped,
    /// so it must not be on screen at a size that triggers the compact path.
    #[test]
    fn compact_drops_the_event_log_first() {
        let rendered = render(&populated(), 60, 18);
        assert!(rendered.contains("tc-npc"), "top bar missing");
        assert!(
            !rendered.contains("イベント"),
            "event log should be dropped in compact mode:\n{rendered}"
        );
    }

    #[test]
    fn config_view_lists_modules_and_flags_the_ones_needing_a_restart() {
        let mut app = populated();
        app.toggle_view();
        let rendered = render(&app, 80, 24);
        assert!(rendered.contains("モジュール"), "config title missing");
        assert!(rendered.contains("[x]"), "enabled marker missing");
        assert!(rendered.contains("[ ]"), "disabled marker missing");
        assert!(
            rendered.contains("要再起動"),
            "restart warning missing — a toggle that silently does nothing \
             until relaunch is exactly what this label exists to prevent:\n{rendered}"
        );
    }

    /// A long error must not evict the transcript: it is capped, so the chat
    /// panel has to still be on screen behind it.
    #[test]
    fn a_long_error_does_not_evict_the_transcript() {
        let mut app = populated();
        app.set_error(
            "非常に長いエラー메시지です。".repeat(40),
            std::time::Instant::now(),
        );
        let rendered = render(&app, 120, 40);
        assert!(rendered.contains("エラー"), "error missing");
        assert!(
            rendered.contains("チャット"),
            "chat panel was pushed off screen by the error:\n{rendered}"
        );
    }
}
