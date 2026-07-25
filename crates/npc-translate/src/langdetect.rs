//! Conservative script-based language guessing, ported from Go
//! `agent-speech`'s `internal/app/langdetect.go`. Only used to decide whether
//! an utterance is in the configured source language or a reply *to* it (see
//! `translation.auto_reverse`), so guessing wrong is cheap — but guessing at
//! all when the text is ambiguous is not, hence the deliberate `None`s.

/// Minimum number of Latin letters before text is confidently classified as
/// English. Shorter runs ("OK", "Hi") are too ambiguous — loanwords and
/// acronyms show up inside Japanese sentences all the time.
const MIN_LATIN_CHARS_FOR_DETECTION: usize = 3;

/// The language labels this detector can return. They are matched against the
/// free-form labels in `config.translation`, so they use the same Japanese
/// wording as the Go original's `config.Lang*` constants.
pub const LANG_JAPANESE: &str = "日本語";
pub const LANG_ENGLISH: &str = "英語";

/// Guess the language of `text`, returning `None` when not reasonably
/// confident: Han-only text is ambiguous between Japanese and Chinese, and
/// text with no alphabetic content at all says nothing.
pub fn detect_language(text: &str) -> Option<&'static str> {
    let mut has_kana = false;
    let mut has_han = false;
    let mut latin_count = 0usize;

    for c in text.chars() {
        if is_kana(c) {
            has_kana = true;
        } else if is_han(c) {
            has_han = true;
        } else if c.is_alphabetic() && c.is_ascii() {
            latin_count += 1;
        }
    }

    if has_kana {
        return Some(LANG_JAPANESE);
    }
    if !has_han && latin_count >= MIN_LATIN_CHARS_FOR_DETECTION {
        return Some(LANG_ENGLISH);
    }
    None
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}')
}

/// CJK Unified Ideographs plus Extension A and the compatibility block —
/// enough to recognize "this is Han script" for the purposes above.
fn is_han(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_means_japanese() {
        assert_eq!(detect_language("こんにちは"), Some(LANG_JAPANESE));
        assert_eq!(detect_language("カタカナ"), Some(LANG_JAPANESE));
        // Kana wins even when Latin and Han are also present.
        assert_eq!(detect_language("OKです、了解"), Some(LANG_JAPANESE));
    }

    #[test]
    fn latin_runs_mean_english() {
        assert_eq!(detect_language("hello there"), Some(LANG_ENGLISH));
    }

    #[test]
    fn short_latin_runs_are_not_confident_enough() {
        assert_eq!(detect_language("OK"), None);
        assert_eq!(detect_language("Hi"), None);
    }

    #[test]
    fn han_only_text_is_ambiguous() {
        // Could be Japanese or Chinese — the Go original refuses to guess too.
        assert_eq!(detect_language("今日会議"), None);
        assert_eq!(detect_language("你好世界"), None);
    }

    #[test]
    fn non_alphabetic_text_says_nothing() {
        assert_eq!(detect_language("123 !!!"), None);
        assert_eq!(detect_language(""), None);
    }
}
