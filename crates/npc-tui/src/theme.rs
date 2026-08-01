//! Color palette and reusable [`Style`] builders for `npc-tui`'s widgets.
//!
//! This crate's visual language is a deliberate translation of
//! `agent-speech`'s bubbletea/lipgloss TUI (see
//! `C:\Projects\tik-choco-lab\agent-speech\internal\tui\model.go`'s
//! `Color*` constants and `view_components.go`'s style helpers) rather than
//! an independent design. Keeping the same hex values and the same small
//! vocabulary of style functions means a contributor who has worked on one
//! of these two Rust/Go TUI clients recognizes the other's building blocks
//! immediately, even though ratatui and lipgloss render very differently
//! under the hood (see `widgets.rs`'s module doc for that difference).
//!
//! Deliberately absent here: any call that paints a *background* over a
//! whole panel or the app frame. `agent-speech` never does that either
//! (lipgloss panels are bordered, not filled) — the terminal's own
//! background (often themed by the user, sometimes transparent) is part of
//! what makes a TUI feel native, and painting over it would fight that.
//! The one exception is [`chip_style`]/`node_style`-shaped badges, which
//! *are* meant to be small blocks of solid color (agent-speech's `chip`/
//! `node`) — that's a deliberately different case from "the panel/app
//! background", not a contradiction of the rule above.

use ratatui::style::{Color, Modifier, Style};

/// Primary brand color. Used for active/selected chrome (focused borders,
/// the "active" node in a flow diagram). Verbatim from agent-speech's
/// `ColorPrimary`.
pub const PRIMARY: Color = Color::Rgb(0x5A, 0x67, 0xD8);

/// Secondary accent, used sparingly for a second category of "on" state
/// that shouldn't compete with [`PRIMARY`] (e.g. a different pipeline
/// stage). Verbatim from `ColorSecondary`.
pub const SECONDARY: Color = Color::Rgb(0x2D, 0xD4, 0xBF);

/// Verbatim from `ColorSuccess`.
pub const SUCCESS: Color = Color::Rgb(0x04, 0xE3, 0x84);

/// Verbatim from `ColorWarning`.
pub const WARNING: Color = Color::Rgb(0xFF, 0xB8, 0x00);

/// Verbatim from `ColorError`.
pub const ERROR: Color = Color::Rgb(0xFF, 0x4B, 0x4B);

/// Verbatim from `ColorGray`. Used for inactive/disabled chrome — the
/// unfocused counterpart to [`PRIMARY`].
pub const GRAY: Color = Color::Rgb(0x3C, 0x3C, 0x3C);

/// Verbatim from `ColorText`. Deliberately *not* used as a blanket default
/// foreground — see the module doc's note on not overriding the terminal's
/// own palette. Reach for it only where a badge/chip needs guaranteed
/// contrast against a colored background, same as agent-speech does.
pub const TEXT: Color = Color::Rgb(0xE1, 0xE1, 0xE1);

/// Verbatim from `ColorMuted`. For de-emphasized text (timestamps, "OFF"
/// labels, secondary annotations).
pub const MUTED: Color = Color::Rgb(0x66, 0x66, 0x66);

/// Verbatim from `ColorHighlight`. Used as the foreground for text sitting
/// on top of a colored chip/node background, where it needs to read
/// clearly regardless of which accent color the badge itself uses.
pub const HIGHLIGHT: Color = Color::Rgb(0xFF, 0xFF, 0xFF);

/// Style for a section heading: bold text in the given accent color.
/// Mirrors agent-speech's `sectionTitle(s, color)`, which is likewise just
/// bold + foreground with no background fill.
pub fn title(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// Style for de-emphasized text. Mirrors agent-speech's `muted(s)`.
pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

/// Style for a "chip"/badge: a solid block of `color` with [`HIGHLIGHT`]
/// text, bold. Mirrors agent-speech's `chip(s, color)`. This is the one
/// place in this module that intentionally sets a background — see the
/// module doc — because a chip's entire visual purpose is to read as a
/// small colored pill, not to blend into the terminal.
pub fn chip_style(color: Color) -> Style {
    Style::new()
        .fg(HIGHLIGHT)
        .bg(color)
        .add_modifier(Modifier::BOLD)
}

/// Style for a flow-diagram node: same solid-badge shape as [`chip_style`],
/// but the color itself carries the meaning (active vs. inactive) rather
/// than being chosen by the caller per label. Mirrors agent-speech's
/// `node(label, active)`, which picks `ColorPrimary`/`ColorGray` the same
/// way. Not bold, matching agent-speech (nodes are meant to sit calmly
/// next to bold `edge()` arrows without competing for attention).
pub fn node_style(active: bool) -> Style {
    let bg = if active { PRIMARY } else { GRAY };
    Style::new().fg(HIGHLIGHT).bg(bg)
}

/// Style for a panel/block border. Unlike `ui.rs`'s existing local
/// `border_style` (which deliberately uses plain ANSI `Cyan`/`DarkGray` for
/// the three hand-built panels that predate this module — not this crate's
/// job to relitigate), this is the palette-driven version for new widgets
/// built on top of this module's vocabulary: [`PRIMARY`] when focused,
/// [`GRAY`] when not, so borders drawn with this helper visually agree with
/// [`node_style`]'s active/inactive coloring.
pub fn border(focused: bool) -> Style {
    if focused {
        Style::new().fg(PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(GRAY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Not much to assert about a color palette beyond "it compiles and the
    // values are what we copied them from" — this pins the hex transcription
    // itself, since a typo'd Rgb triple would otherwise only show up as a
    // subtly wrong color nobody notices for months.
    #[test]
    fn palette_matches_agent_speech_hex_values() {
        assert_eq!(PRIMARY, Color::Rgb(0x5A, 0x67, 0xD8));
        assert_eq!(SECONDARY, Color::Rgb(0x2D, 0xD4, 0xBF));
        assert_eq!(SUCCESS, Color::Rgb(0x04, 0xE3, 0x84));
        assert_eq!(WARNING, Color::Rgb(0xFF, 0xB8, 0x00));
        assert_eq!(ERROR, Color::Rgb(0xFF, 0x4B, 0x4B));
        assert_eq!(GRAY, Color::Rgb(0x3C, 0x3C, 0x3C));
        assert_eq!(TEXT, Color::Rgb(0xE1, 0xE1, 0xE1));
        assert_eq!(MUTED, Color::Rgb(0x66, 0x66, 0x66));
        assert_eq!(HIGHLIGHT, Color::Rgb(0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn node_style_switches_background_on_active() {
        assert_eq!(node_style(true).bg, Some(PRIMARY));
        assert_eq!(node_style(false).bg, Some(GRAY));
    }

    #[test]
    fn border_bolds_only_when_focused() {
        assert!(border(true).add_modifier.contains(Modifier::BOLD));
        assert!(!border(false).add_modifier.contains(Modifier::BOLD));
    }
}
