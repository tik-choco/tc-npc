//! Rendering only — every value this module reads comes straight off [`App`]
//! and nothing here mutates it. Kept separate from `app.rs` so the state
//! transitions there stay testable without a terminal (see that module's
//! doc comment), and separate from `lib.rs` so the event loop isn't tangled
//! up with layout code.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ChatRow, ConnectionState, Focus};

const FOCUS_BORDER: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const IDLE_BORDER: Style = Style::new().fg(Color::DarkGray);

fn border_style(focused: bool) -> Style {
    if focused {
        FOCUS_BORDER
    } else {
        IDLE_BORDER
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status bar
            Constraint::Length(1), // error/status banner (blank when idle)
            Constraint::Min(3),    // chat + log body
            Constraint::Length(3), // input box
            Constraint::Length(1), // key-binding help
        ])
        .split(area);

    draw_status_bar(frame, rows[0], app);
    draw_banner(frame, rows[1], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        // 65/35: the chat transcript is the thing an operator actually
        // reads turn-by-turn, the log panel is secondary context (sense/
        // memory/action-log/unmodeled frames) — see the worker brief's
        // scope list ordering chat before log for the same reason.
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(rows[2]);

    draw_chat(frame, body[0], app);
    draw_log(frame, body[1], app);

    draw_input(frame, rows[3], app);
    draw_help(frame, rows[4]);
}

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let conn_color = match app.connection {
        ConnectionState::Connected => Color::Green,
        ConnectionState::Connecting | ConnectionState::Reconnecting => Color::Yellow,
        ConnectionState::Disconnected => Color::Red,
    };

    let mut spans = vec![
        Span::styled(
            app.connection.label(),
            Style::new().fg(conn_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];

    if let Some(version) = &app.version {
        spans.push(Span::raw(format!("v{version}  ")));
    }

    match &app.character {
        Some(character) => spans.push(Span::raw(format!("キャラ: {}  ", character.name))),
        None => spans.push(Span::styled(
            "キャラ: (未設定)  ",
            Style::new().fg(Color::DarkGray),
        )),
    }

    // Module flags: each name colored green when enabled, dark gray when
    // not, rather than a longer on/off word per flag — this line has to fit
    // seven of them plus everything above on one row.
    let flags: [(&str, bool); 7] = [
        ("talk", app.modules.talk),
        ("memory", app.modules.memory),
        ("speech", app.modules.speech),
        ("vision", app.modules.vision),
        ("action", app.modules.action),
        ("scheduler", app.modules.scheduler),
        ("translation", app.modules.translation),
    ];
    for (name, on) in flags {
        let color = if on { Color::Green } else { Color::DarkGray };
        spans.push(Span::styled(name, Style::new().fg(color)));
        spans.push(Span::raw(" "));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_banner(frame: &mut Frame, area: Rect, app: &App) {
    let text = match &app.status_line {
        Some(line) => Line::from(Span::styled(line.as_str(), Style::new().fg(Color::Red))),
        None => Line::from(""),
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_chat(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("チャット")
        .borders(Borders::ALL)
        .border_style(border_style(app.focus == Focus::Input));

    let mut lines: Vec<Line> = Vec::with_capacity(app.chat.len() + 1);
    for row in &app.chat {
        lines.push(match row {
            ChatRow::Chat { role, text } if role == "assistant" => Line::from(vec![
                Span::styled(
                    "NPC: ",
                    Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                ),
                Span::raw(text.clone()),
            ]),
            ChatRow::Chat { text, .. } => Line::from(vec![
                Span::styled(
                    "You: ",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw(text.clone()),
            ]),
            ChatRow::Silent { reason } => Line::from(Span::styled(
                format!("(反応なし: {reason})"),
                Style::new()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
        });
    }
    if app.pending {
        lines.push(Line::from(Span::styled(
            "NPC: ...",
            Style::new().fg(Color::DarkGray),
        )));
    }

    render_scrollable(frame, area, block, lines, app.chat_scroll);
}

fn draw_log(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title("ログ")
        .borders(Borders::ALL)
        .border_style(border_style(app.focus == Focus::Log));

    let lines: Vec<Line> = app
        .log
        .iter()
        .map(|row| {
            Line::from(vec![
                Span::styled(format!("[{}] ", row.label), Style::new().fg(Color::Yellow)),
                Span::raw(row.text.clone()),
            ])
        })
        .collect();

    render_scrollable(frame, area, block, lines, app.log_scroll);
}

/// Shared rendering for both scrollable panels: word-wraps `lines` inside
/// `block`, then bottom-anchors the viewport and scrolls it up by
/// `scroll_up` lines from there.
///
/// `Paragraph::line_count` (added specifically so callers can build their
/// own scrollbar-style logic on top of it) is what makes an exact
/// bottom-anchor possible without hand-rolling unicode-width-aware wrapping
/// here: it reports how many lines `text` reflows to at a given width, using
/// the exact same wrapping `Wrap { trim: false }` will use to render it, so
/// the count this function computes and the layout the widget actually
/// draws can never disagree.
fn render_scrollable(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    lines: Vec<Line>,
    scroll_up: u16,
) {
    let inner = block.inner(area);
    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });

    let total_lines = paragraph.line_count(inner.width) as u16;
    let max_scroll = total_lines.saturating_sub(inner.height);
    // `scroll_up` is "how many lines above the auto-follow position", not an
    // absolute line index — see `App::chat_scroll`/`log_scroll`'s doc — so a
    // value larger than `max_scroll` (e.g. after `App` saturates it well past
    // the top on a fast PageUp) is simply clamped here rather than treated as
    // an error; nothing needs to feed the clamped value back into `App`.
    let scroll = max_scroll.saturating_sub(scroll_up.min(max_scroll));

    frame.render_widget(paragraph.block(block).scroll((scroll, 0)), area);
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Input;
    let block = Block::default()
        .title("入力 (Enter: 送信)")
        .borders(Borders::ALL)
        .border_style(border_style(focused));

    // A trailing block cursor glyph rather than an actual terminal cursor
    // position: this crate never calls `Terminal::show_cursor`/`set_cursor`,
    // so a plain text cursor is simpler than wiring up real cursor
    // placement for a single-line box with no mid-line editing (see
    // `App::input`'s doc — v1 only supports append/backspace, not arrow-key
    // cursor movement within the line).
    let mut text = app.input.clone();
    if focused {
        text.push('_');
    }

    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let help = "Tab: フォーカス切替  Enter: 送信  ↑/↓/PgUp/PgDn: スクロール  Esc/Ctrl+C: 終了";
    frame.render_widget(
        Paragraph::new(Span::styled(help, Style::new().fg(Color::DarkGray))),
        area,
    );
}
