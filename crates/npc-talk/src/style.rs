//! Spoken-reply discipline: the built-in reply rules handed to the model,
//! and the sanitizer that cleans up what comes back.
//!
//! Both exist because a reply here is *spoken* — npc-speech reads
//! `chat_response` straight out to TTS — while an instruction-tuned model
//! left to itself writes prose: stage directions in parentheses ("（ふっと
//! 視線を緩め、静かに口角を上げる）"), markdown, and written-register
//! sentences that sound wrong read aloud.
//!
//! `talk.prompts` is user-editable and defaults to *empty*, so rules kept
//! only in the config file are rules the NPC routinely runs without. The
//! prompt block below therefore lives in the binary and is injected on every
//! turn (`talk.style_rules`, default on). The sanitizer is the second half of
//! the same job: small local models leak stage directions no matter how the
//! prompt is worded, so the leaked markup is stripped before the reply is
//! published, spoken, or written to history.
//!
//! Rule wording is adapted from the conversation-quality work in
//! `tik-choco-lab/archives/agent-conversation` (`src/agent.py`'s reply rules
//! and `doc/conversation-quality.md`'s evaluation axes: turn-taking,
//! engagement, grounding, metacognition, closure).

use std::sync::OnceLock;

use regex::Regex;

/// Built-in conversation rules, pushed as a system message after the
/// configured `talk.prompts` and before the affect state.
///
/// Written in Japanese to match the rest of this app's prompts
/// (`npc_core::persona_prompt`, the memory prompts). The reply *language* is
/// not decided here — `config.language`'s instruction is pushed after this
/// block and wins.
pub const SPOKEN_REPLY_RULES: &str = "\
これは声に出して交わされる会話です。返答は、そのまま読み上げられる話し言葉だけにしてください。

出力の形:
・出力はあなたが実際に口に出すセリフだけ。地の文・ト書き・状況説明を書かない。
・「（ふっと視線を緩める）」「（微笑む）」「*ため息*」のような、括弧や記号で囲んだ仕草・表情・心情の描写は一切書かない。伝えたい態度は言葉そのもので示すか、書かずに省く。
・見出し、箇条書き、番号付け、Markdown記法（**、##、``` など）、絵文字、顔文字を使わない。
・話者名や「〇〇:」のような前置きを付けない。返答全体を引用符で囲まない。
・改行しない。ひと続きの発話として言い切る。
・書き言葉の言い回し（「〜であるということでしょう」「〜という点において」）を避け、声に出して自然な言い方を選ぶ。

話し方:
・短く。基本は1文、長くても2文。だらだら続けない。
・相手の直前の発言に具体的に反応する。一般論や前置きで埋めない。
・比喩・詩的表現・大げさな情景描写を多用しない。日常会話のテンポで話す。
・感嘆詞や笑い表現、過剰な称賛で感情を盛らない。嬉しさや興味は語の選び方で静かに出す。
・毎回あいさつや相手の名前を繰り返さない。会話の流れを優先する。
・「はい」「うん」「なるほど」のような短い受けを適度に混ぜる。ただし毎回同じ相づちにしない。
・相手の感情とテンポに合わせる。疲れている・眠いと言われたら励まし立てず、静かに合わせる。

話の運び:
・直前までの話題と前提を保って続ける。話を勝手に飛ばさない。
・「それ」「あれ」などの指示語は、指すものが相手と自分の間で一意に分かる時だけ使う。曖昧なら具体名で言い直す。
・推測が外れたら取り繕わず、短く認めて言い直す。
・質問攻めにしない。問いを続けざまに出さず、まず相手の言葉を受け止める。
・記憶は必要な時だけさりげなく反映する。覚えていないことを覚えているふりはしない。
・記録に無い出来事や、確かめていないことを、あったことのように話さない。分からないことは「分かりません」と正直に言う。
・話題が一段落したら無理に引き延ばさない。相手の返事が短い受けだけになったら、こちらも短く受けて閉じる。
・別れの挨拶には短い別れの言葉だけを返す。新しい話題・次を促す言葉・感傷的な余韻を足さない。

黙る:
・毎回必ず応答しなくてよい。何も言わない方が自然な時は、<silence> とだけ返す。その回は本当に何も言わない（「……」や相づちも出さない）。
・<silence> を選ぶのは例えばこんな時: 会話がもう終わっている／別れの挨拶を返した後に「うん」「はい」だけが続く／自分に向けられていない他の人同士の会話や独り言が聞こえた／聞き取りが断片的で何を言われたのか分からない／今は黙って聞いている方がいい場面。
・逆に、自分に話しかけられている・名前を呼ばれた・質問された時は黙らない。迷ったら短く応じる。
・<silence> を返す時は、それだけを出力する。説明や理由を添えない。";

/// What the model outputs to take a turn without speaking. Deliberately an
/// ASCII tag rather than a natural phrase: nothing a character would ever
/// say aloud can collide with it, so a bare `contains` check is safe.
pub const SILENCE_SENTINEL: &str = "<silence>";

/// Whether a (already sanitized) reply means "say nothing this turn".
///
/// Three ways to land here, all treated the same: the explicit
/// [`SILENCE_SENTINEL`], a reply that is empty once the markup is stripped,
/// and a reply that is nothing but ellipsis/punctuation — which is what both
/// the closing state machine's fixed "……" and a model trailing off produce.
/// Speaking those aloud gives TTS a line with no words in it and puts an
/// empty bubble in the transcript; staying quiet is what they actually mean.
pub fn is_silence(text: &str) -> bool {
    let text = text.trim();
    if text.contains(SILENCE_SENTINEL) {
        return true;
    }
    // No alphanumeric and no CJK left => nothing to pronounce.
    !text.chars().any(|c| c.is_alphanumeric())
}

/// Strip everything that shouldn't be read aloud (or shown as a chat bubble)
/// out of one reply: parenthesized stage directions, asterisk roleplay
/// markup, markdown line markers, and stray blank lines.
///
/// A reply that was *entirely* stage direction comes back empty, which
/// [`is_silence`] reads as "say nothing this turn" — the character wasn't
/// speaking in the first place, so narrating "（少し黙る）" out loud is the
/// wrong repair.
pub fn sanitize_reply(text: &str) -> String {
    normalize_lines(&strip_markup(text))
}

fn strip_markup(text: &str) -> String {
    static FULLWIDTH_PAREN: OnceLock<Regex> = OnceLock::new();
    static HALFWIDTH_PAREN: OnceLock<Regex> = OnceLock::new();
    static BOLD: OnceLock<Regex> = OnceLock::new();
    static EMPHASIS: OnceLock<Regex> = OnceLock::new();
    static LINE_MARKER: OnceLock<Regex> = OnceLock::new();
    static FENCE: OnceLock<Regex> = OnceLock::new();

    // Full-width parentheses in a spoken line are stage directions
    // essentially every time they appear, so they go unconditionally.
    let out = FULLWIDTH_PAREN
        .get_or_init(|| Regex::new(r"（[^（）]*）").unwrap())
        .replace_all(text, "");

    // Half-width ones are not: they carry real content in mixed Japanese/
    // ASCII text ("(v2)", "(9時)"). Only a span that is pure prose — CJK
    // present, no letters or digits — reads as the same stage direction
    // written with the other keyboard.
    let out = HALFWIDTH_PAREN
        .get_or_init(|| Regex::new(r"\(([^()\n]*)\)").unwrap())
        .replace_all(&out, |caps: &regex::Captures| {
            let inner = &caps[1];
            if has_cjk(inner) && !inner.chars().any(|c| c.is_ascii_alphanumeric()) {
                String::new()
            } else {
                caps[0].to_string()
            }
        });

    // `**強調**` is markdown emphasis around real speech — keep the words,
    // drop the markers. A single-asterisk span is the roleplay-action
    // convention (`*sighs*`) and goes whole.
    let out = BOLD
        .get_or_init(|| Regex::new(r"\*\*([^*\n]+)\*\*").unwrap())
        .replace_all(&out, "$1");
    let out = EMPHASIS
        .get_or_init(|| Regex::new(r"\*[^*\n]*\*").unwrap())
        .replace_all(&out, "");

    let out = FENCE
        .get_or_init(|| Regex::new(r"(?m)^\s*```.*$").unwrap())
        .replace_all(&out, "");
    let out = LINE_MARKER
        .get_or_init(|| Regex::new(r"(?m)^\s*(?:#{1,6}\s+|[-*+>]\s+|\d+[.)]\s+)").unwrap())
        .replace_all(&out, "");

    out.into_owned()
}

/// Trim the reply, drop blank lines, and squeeze runs of spaces — the chat
/// bubble renders `white-space: pre-wrap`, so a leading newline or a blank
/// line between sentences shows up as dead space in every message.
fn normalize_lines(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let squeezed = squeeze_spaces(line.trim());
        if !squeezed.is_empty() {
            lines.push(squeezed);
        }
    }
    lines.join("\n")
}

fn squeeze_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_space = false;
    for c in line.chars() {
        // Full-width space included: it survives `trim` inside a line and is
        // what a model that padded a stripped stage direction leaves behind.
        if c == ' ' || c == '\t' || c == '\u{3000}' {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim().to_string()
}

fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{3040}'..='\u{309f}' // hiragana
            | '\u{30a0}'..='\u{30ff}' // katakana
            | '\u{4e00}'..='\u{9fff}' // CJK unified ideographs
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_stage_direction_that_started_all_this() {
        let reply = "（ふっと視線を緩め、あなたの本音に寄り添うように静かに口角を上げる）\n\n……そうですね。";
        assert_eq!(sanitize_reply(reply), "……そうですね。");
    }

    #[test]
    fn strips_a_trailing_stage_direction_too() {
        assert_eq!(sanitize_reply("はい。（微笑む）"), "はい。");
    }

    #[test]
    fn strips_halfwidth_parens_only_when_they_read_as_prose() {
        assert_eq!(sanitize_reply("はい。(小さく頷く)"), "はい。");
        // Real content in half-width parens stays put.
        assert_eq!(sanitize_reply("v2(beta)を試します。"), "v2(beta)を試します。");
        assert_eq!(sanitize_reply("(9時)に始めます。"), "(9時)に始めます。");
    }

    #[test]
    fn drops_roleplay_asterisks_but_keeps_emphasised_speech() {
        assert_eq!(sanitize_reply("*ため息* わかりました。"), "わかりました。");
        assert_eq!(sanitize_reply("**本当に**そう思います。"), "本当にそう思います。");
    }

    #[test]
    fn removes_blank_lines_and_leading_whitespace() {
        assert_eq!(sanitize_reply("\n\nこんにちは。\n\nお元気ですか。\n"), "こんにちは。\nお元気ですか。");
        assert_eq!(sanitize_reply("  はい。  "), "はい。");
        assert_eq!(sanitize_reply("はい。　　そうですね。"), "はい。 そうですね。");
    }

    #[test]
    fn strips_markdown_line_markers_and_fences() {
        assert_eq!(sanitize_reply("- はい。\n- そうですね。"), "はい。\nそうですね。");
        assert_eq!(sanitize_reply("## 返答\nはい。"), "返答\nはい。");
        assert_eq!(sanitize_reply("```\nはい。\n```"), "はい。");
    }

    #[test]
    fn a_reply_that_is_only_a_stage_direction_becomes_silence() {
        // The character never said anything — reading the direction out loud
        // would be the wrong repair, so the turn goes quiet instead.
        assert_eq!(sanitize_reply("\n（微笑む）\n"), "");
        assert!(is_silence(&sanitize_reply("\n（微笑む）\n")));
    }

    #[test]
    fn ordinary_speech_is_untouched() {
        let reply = "ほうじ茶、昨日も飲んでましたね。";
        assert_eq!(sanitize_reply(reply), reply);
        // Japanese quotation marks are speech, not markup.
        assert_eq!(sanitize_reply("「はい」と答えました。"), "「はい」と答えました。");
    }

    #[test]
    fn silence_is_recognised_however_the_model_spells_it() {
        assert!(is_silence(SILENCE_SENTINEL));
        assert!(is_silence(" <silence> "));
        assert!(is_silence(""));
        assert!(is_silence("……"));
        assert!(is_silence("...")); // the closing state machine's fixed reply
        assert!(is_silence("。"));
    }

    #[test]
    fn anything_with_words_in_it_is_not_silence() {
        assert!(!is_silence("はい。"));
        assert!(!is_silence("……そうですね。"));
        assert!(!is_silence("ok"));
    }

    #[test]
    fn a_sentinel_survives_sanitizing_so_the_engine_can_still_see_it() {
        // The sanitizer runs first; it must not eat the tag (or leave
        // something that stops reading as silence).
        assert!(is_silence(&sanitize_reply("<silence>")));
        assert!(is_silence(&sanitize_reply("（少し黙る）")));
    }

    #[test]
    fn rules_tell_the_model_how_to_stay_quiet() {
        assert!(SPOKEN_REPLY_RULES.contains(SILENCE_SENTINEL));
        assert!(SPOKEN_REPLY_RULES.contains("毎回必ず応答しなくてよい"));
    }

    #[test]
    fn rules_still_carry_the_constraints_the_sanitizer_backs_up() {
        assert!(SPOKEN_REPLY_RULES.contains("ト書き"));
        assert!(SPOKEN_REPLY_RULES.contains("改行しない"));
        assert!(SPOKEN_REPLY_RULES.contains("Markdown"));
    }
}
