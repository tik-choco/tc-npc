//! Emotion-drive model and closing-turn state machine, ported from the
//! reference TypeScript implementation `tc-assistant2/src/conversation.ts`
//! (`ConversationAffectState`, lines 170-327, plus the vocabulary constants
//! at the top of that file). Memory and persona handling live elsewhere in
//! this workspace (`npc-memory`, persona prompts) and are *not* touched
//! here — this module is purely the internal "drive" state and the
//! farewell/closing state machine layered on top of it.
//!
//! Numeric constants (bases, deltas, decay pulls, thresholds) started as a
//! verbatim copy from the TypeScript original; see the doc comments on
//! individual items for the corresponding line ranges. The vocabulary lists
//! and a handful of per-word deltas have since diverged from that original
//! — see the second paragraph below the integrator note for why.
//!
//! One piece of behavior is deliberately *not* a verbatim port: the
//! per-turn integration step in [`AffectState::update`] is a bounded
//! integrator, not the original's unbounded `level += (base - level) *
//! pull; level += delta`. The unbounded version has an ugly property —
//! several drives' `pull` values are small enough that ordinary
//! conversation drives their fixed point past 1.0, so a handful of turns
//! of unremarkable dialogue pins them to the ceiling for the rest of the
//! session (measured: dopamine, oxytocin, acetylcholine and orexin all
//! reach exactly `1.0` within ~15 turns of plain 16+-character sentences).
//! Once pinned, the drive stops carrying information — a rapturous turn
//! and a flat one both read as `1.0` — and [`AffectState::to_prompt`]'s
//! top-3-by-deviation salience ends up repeating the same three lines
//! every turn thereafter, which is as much a bug in the character's
//! *described* internal state as it is in its expression. See the comment
//! on the integration loop itself for the replacement and why it doesn't
//! have this failure mode. At the time that change landed, every constant
//! — bases, deltas, pulls, vocabulary — was still copied verbatim; only
//! the integration step differed.
//!
//! That is no longer true of the vocabulary lists (`POSITIVE`, `CONFLICT`,
//! the new `SURPRISE`, etc.) or of a few of the deltas that read them.
//! Measured against the 160-turn labelled eval set
//! (`eval/emotion/dataset.jsonl`, scored through `web/src/lib/
//! vrm-emotion.ts`'s mapping from these 22 drives onto six VRM
//! expressions), the verbatim-ported vocabulary produced almost no usable
//! signal for four of those six: happy recall 31.6%, sad recall 8.3%,
//! angry recall 11.1%, surprised recall 0.0%. The original word lists
//! simply don't cover how an ordinary Japanese conversation expresses
//! being pleased, hurt, angry or startled — `POSITIVE` had nothing for
//! "褒められた"/"助かった"/"よかった", `adrenaline` only fired on a
//! conflict word paired with a question mark (so a flat "最悪だよ、ひどい
//! よ" never moved it), and no list at all tracked surprise. The
//! vocabulary below extends those lists with short stems chosen to absorb
//! ordinary inflections — the same approach the original lists already
//! used for e.g. "楽し", "疲", "痛" — with an explicit note wherever an
//! addition risks matching an unrelated word (`contains`-based matching
//! makes that a real risk, not a theoretical one: the original's bare
//! "嫌" already matched "機嫌" — mood — before this pass replaced it with
//! more specific inflections). This is scoped narrowly: the bases, the
//! 0.08/0.15/0.2 pulls, and the bounded integrator above are all still
//! exactly as before.
//!
//! A later, narrower pass targeted the three endogenous-opioid drives
//! (`endorphin`, `enkephalin`, `dynorphin`) plus `anandamide` (an
//! endocannabinoid, not an opioid, but sharing endorphin's `happy` vote in
//! vrm-emotion.ts). All four were wired to a mapping vote but almost never
//! fired: measured fire counts against the 288-turn eval set were
//! endorphin 19, enkephalin 9, dynorphin 12, and anandamide 1 (out of a
//! possible 288 each) — anandamide's condition was a logical AND of two
//! clauses (`positive && calm`) that essentially never co-occur in the
//! same utterance, a dead vote in all but name. See each drive's own delta
//! comment below for what was tried and measured. Net result: enkephalin's
//! fire count rose to 43 and anandamide's to 23, relaxed recall rose from
//! 26.7% to 30.0% and happy recall from 29.3% to 32.8%, all with neutral
//! recall *unchanged* at 61.3% and dev strict accuracy rising from 49.7%
//! to 50.3% (holdout 47.5% -> 49.5%, dev-holdout gap actually narrowing
//! from +2.3pp to +0.8pp). Two of the four had no safe headroom: raising
//! `endorphin`'s `humor`/`positive` weights and raising `dynorphin` past
//! 0.2 each reproduced a version of the same failure this file's other
//! comments already document elsewhere (POSITIVE's ubiquity, dynorphin's
//! shared trigger with cortisol/substance_p) — both were left at their
//! pre-pass values after being measured, not merely assumed, to not be
//! worth it.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;

// ---------------------------------------------------------------------
// vocabulary constants (conversation.ts lines 42-57)
// ---------------------------------------------------------------------

// The lists below are no longer a verbatim port — see the module doc comment
// for the measured recall numbers that motivated extending them. Every
// addition is a short stem chosen to absorb ordinary inflections (the
// approach the original lists already used for e.g. "楽し", "疲", "痛"),
// with a note wherever a stem risks matching an unrelated word.

const BOND: &[&str] = &[
    "私", "僕", "俺", "です", "好き", "嬉し", "よろしく", "ありがとう", "君", "あなた",
    // Trust/closeness words the pronoun-heavy original list didn't cover.
    "信頼", "大切", "頼りに",
];
// "嬉しい" narrowed to the "嬉し" stem (matches "嬉しかった"/"嬉しくて" too — the exact form
// alone was missing most of its own inflections). The rest of the additions are the specific
// gap the mapping diagnosis named: praise, relief, gratitude, being helped, admiration.
// "すごい" and "安心" can in principle modify a negative ("すごい嫌だ"), but both read as
// positive in the overwhelming majority of ordinary use, and an occasional false fire here is
// absorbed by the mapping's DEVIATION_FLOOR plus its bias toward neutral on a single weak
// signal.
const POSITIVE: &[&str] = &[
    "はい", "なるほど", "ありがとう", "いいね", "嬉し", "そうですね", "うん", "楽し",
    "よかった", "良かった", "助か", "すごい", "最高", "感謝", "褒め", "安心",
];
// The original's bare "嫌" matches "機嫌" ("kigen", mood) as a substring — "機嫌がいい" (in a
// good mood) would have read as conflict. Replaced with specific inflections that don't have
// that collision. Added common angry/confrontational phrasing the original missed entirely
// ("ひどい", "腹立つ", "うざい") — this is what lets a flat, non-question conflict statement
// ("最悪だよ、ひどいよ") register at all; see the `adrenaline` delta below for why that
// mattered.
const CONFLICT: &[&str] = &[
    "嫌い", "嫌だ", "嫌な", "違う", "うるさい", "やめて", "怒", "むかつく", "ムカつ", "バカ", "馬鹿", "最悪",
    "ひどい", "腹立", "うざ",
];
const HUMOR: &[&str] = &["笑", "ふふ", "ｗ", "w", "面白", "楽し", "冗談"];
// "元気が出な" added on top of the original TIRED list: covers the very common "元気が出ない"
// / "元気が出なくて" idiom for low motivation/energy — otherwise that whole sentence would
// register nothing here while RECOVER's "元気" fires the opposite (positive) signal, since
// `contains` can't see that it's negated. This doesn't cancel that false-positive-by-negation
// (RECOVER's "元気" still can't tell "元気が出ない" from "元気になった"), but it does at least
// give TIRED/PAIN a competing signal on the same sentence instead of none.
//
// "気力" added: sad turns in the eval set routinely describe low motivation without ever using
// a PAIN word ("声にする気力もあんまりなくて" — "I don't even have the energy to speak up").
// Bare "気力" (rather than a negated stem like "元気が出な" above) is deliberate: in ordinary
// conversational Japanese people mention 気力 almost exclusively to say they lack it ("気力が
// ない"/"気力が湧かない"/"気力もなくて"); declaring an abundance of it ("気力に満ちている") is
// a much rarer construction, so the asymmetry makes the bare stem a reasonable bet the same way
// the pre-existing "しんど"/"だる" stems are.
const TIRED: &[&str] = &["疲", "眠", "休", "寝", "だる", "しんど", "バテ", "くたくた", "元気が出な", "気力"];
const NIGHT: &[&str] = &["夜", "深夜", "暗", "星空", "寝る"];
// "痛" alone already covers 痛い/痛かった. Added:
//   - "辛い" (kanji; distinct from the "つら" stem already here, which only matches the
//     hiragana spelling)
//   - "落ち込" (落ち込んだ/でいる)
//   - "泣" (泣いた/泣ける/泣きたい) — a single kanji, but, like the pre-existing single-kanji
//     "痛"/"怒" stems, rare enough outside this sense that the false-positive risk is low
//   - "不安" (anxiety/worry) — the original list had "不快" (discomfort) but nothing for plain
//     worry, a very common way distress shows up in conversation
//   - "崩" (single kanji: 体調を崩す/崩れる, health or plans falling apart) — like "泣"/"痛",
//     the false-positive surface (雪崩, 崩壊) is rare in ordinary conversation and still reads
//     negative even when it appears
//
// "ごめん" (sorry/apologetic) was tried and dropped: measured against
// eval/emotion/trace.json it fired on plain, mildly-apologetic-but-otherwise-settled turns
// ("気にしすぎたかも、ごめんね") that are genuinely neutral, not sad, costing more neutral
// recall than it gained in sad recall. An apology alone isn't a reliable sad signal the way
// "痛い"/"寂しい"/"不安" are — most everyday "ごめん"s are polite friction-smoothing, not
// distress.
// "笑えな"/"前を向けな"/"名残惜し" added: several eval-set sad turns describe the feeling
// entirely without any word already in this list ("うまく笑えなくて", "うまく前を向けなくて"),
// so no amount of retuning the deltas below can reach them without the vocabulary covering more
// than literal pain/anxiety words. Each is a stem for "this kind of expression", not a copy of
// the dataset's exact sentence:
//   - "笑えな" only matches the negated form ("笑えない"/"笑えなくて") and not "笑える", so it
//     can't collide with a genuinely positive "楽しくて笑える" the way a bare "笑" stem would.
//   - "前を向けな" is the same negation trick: matches "前を向けない"/"前を向けなくて" but not
//     the recovering "前を向けるようになった", which is closer to RECOVER's territory than PAIN's.
//   - "名残惜し" (wistful about parting) doesn't have a realistic positive-valence reading —
//     unlike "手放す" (letting go of something), which was tried and dropped here because it
//     collides with genuine relief ("重荷を手放せてよかった").
const PAIN: &[&str] = &[
    "痛", "つら", "苦し", "不快", "悲し", "寂し", "辛い", "落ち込", "泣", "不安", "崩", "笑えな", "前を向けな",
    "名残惜し",
];
const CALM: &[&str] = &["静", "ゆっくり", "落ち着", "のんびり", "穏やか", "リラックス", "まったり"];
// "！"/"!" used to be in this list, which was a bug: `urgent` feeds adrenaline and cortisol
// (see the deltas below), so *any* exclamation mark — a happy "褒められたんだ!", a surprised
// "えっ、うそでしょ!" — read as urgency and pulled the expression toward angry. Measured: the
// eval set's "聞いて聞いて、今日の発表、すごく褒められたんだ!" (a happy turn) predicted angry
// before this change. An exclamation mark encodes the *intensity* of an utterance, not its
// urgency, and intensity is orthogonal to valence — ideally it would amplify whichever emotion
// is already present rather than being dropped outright.
//
// Two fixes were on the table: drop it from URGENT (this one), or keep the signal by having it
// scale up whichever content-driven deltas already fired this turn (bigger endorphin on an
// exclaimed happy line, bigger adrenaline on an exclaimed angry one). The amplifier is the more
// semantically complete of the two, but it is also a second free parameter (how much to scale
// by) that itself needs tuning and its own dev/holdout drift check, for a case this dataset
// barely exercises — a plain "positive line, no exclamation" and "positive line, with
// exclamation" mostly already land on the same side of every threshold in vrm-emotion.ts once
// the content words alone are voting. Simple removal was measured to fix the target
// misclassifications (this comment's example, and the equivalent surprised/angry cases) without
// the regressions the sad-vocabulary changes below triggered when pushed too far, so it's what
// shipped. If a future pass has eval turns that actually distinguish "said flatly" from "said
// with real intensity" within the same emotion, the amplifier is the more correct next step.
const URGENT: &[&str] = &["急", "早く", "すぐ", "危", "助け", "大変"];
// Added "理解"/"納得" (synonyms of "わかった"/"なるほど") and "気づ" (気づいた/気づく).
const LEARN: &[&str] =
    &["なぜ", "どうして", "つまり", "わかった", "なるほど", "考え", "知り", "教え", "理解", "気づ", "納得"];
const COMFORT: &[&str] = &["大丈夫", "安心", "ありがとう", "ゆっくり", "無理しない", "気にしないで"];
const RECOVER: &[&str] = &["大丈夫", "復活", "元気", "持ち直", "平気", "回復", "立ち直"];
// New: nothing in the original vocabulary tracked surprise at all (surprised recall was
// 0.0% — see the module doc comment). Feeds `acetylcholine`/`orexin`/`histamine` in `update`
// below — the three drives `vrm-emotion.ts`'s PRIMARY_VOTES reads for the `surprised`
// expression (checked there before wiring this up, per the task brief). Candidates considered
// and rejected, with why:
//   - bare "え、": almost any word ending in "え" followed by a comma matches too (e.g. "その
//     考え、"), so it would fire on ordinary sentence rhythm rather than surprise. "えっ" (the
//     small-tsu spelling) is the distinctively-surprised form and doesn't have that collision.
//   - "本当に"/"ほんと": both are plain intensifiers used constantly outside of surprise
//     ("本当にありがとう"); including them would fire on ordinary emphatic thanks/agreement.
//   - "初めて" ("for the first time"): as often a neutral statement of fact as a surprised
//     reaction ("初めて会った日" implies nothing about surprise).
//   - "そんな": far too generic ("そんな感じ", "そんなことない") to carry signal alone.
//   - bare "うそ"/"うわ": "うそ" alone also means "a lie" outside the surprised-exclamation
//     use ("それはうそだ" is an accusation, not surprise); "うわ" is a substring of "うわさ"
//     (rumor). Narrowed to "うそでしょ"/"嘘でしょ" and "うわっ", neither of which has either
//     collision.
const SURPRISE: &[&str] =
    &["えっ", "まさか", "びっくり", "驚", "うそでしょ", "嘘でしょ", "マジ", "意外", "衝撃", "信じられない", "うわっ"];
const LURE: &[&str] = &[
    "行こう", "行きましょう", "おいで", "ついてき", "ついて来", "一緒に", "二人で", "デート", "こっち来", "こっちおいで", "来て",
    "来ない", "連れて", "付いて",
];
const FAREWELL: &[&str] = &[
    "じゃあね", "またね", "また明日", "また今度", "さよなら", "さようなら", "おやすみ", "バイバイ", "ばいばい", "失礼します",
    "失礼するね", "いってきます", "行ってきます", "そろそろ", "もう行く", "切るね", "またあとで", "また後で",
];
const GREETING: &[&str] = &[
    "ただいま", "おかえり", "こんにちは", "こんばんは", "おはよう", "やあ", "ねえ", "ところで", "聞いて", "あのね", "あのさ", "実は",
];
const MINIMAL_ACK: &[&str] = &["はい", "うん", "ええ", "そう", "そっか", "なるほど", "了解", "わかった"];

// ---------------------------------------------------------------------
// drives (conversation.ts lines 59-166)
// ---------------------------------------------------------------------

/// The 22 internal "drives" (loosely: neurotransmitter/hormone levels)
/// tracked per conversation. Order matches the TypeScript `DRIVES` object's
/// key order, and doubles as each variant's index into [`DRIVES`] /
/// [`AffectState::levels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriveKey {
    Dopamine,
    Serotonin,
    Oxytocin,
    Endorphin,
    Cortisol,
    Noradrenaline,
    Adrenaline,
    Acetylcholine,
    Glutamate,
    Gaba,
    Glycine,
    Melatonin,
    Orexin,
    Histamine,
    Dynorphin,
    Dhea,
    Enkephalin,
    Anandamide,
    SubstanceP,
    Npy,
    Cck,
    Bdnf,
}

const DRIVE_COUNT: usize = 22;

/// All drive keys, in declaration order (== [`DriveKey`] discriminant
/// order, == [`DRIVES`] order).
pub const ALL_DRIVES: [DriveKey; DRIVE_COUNT] = [
    DriveKey::Dopamine,
    DriveKey::Serotonin,
    DriveKey::Oxytocin,
    DriveKey::Endorphin,
    DriveKey::Cortisol,
    DriveKey::Noradrenaline,
    DriveKey::Adrenaline,
    DriveKey::Acetylcholine,
    DriveKey::Glutamate,
    DriveKey::Gaba,
    DriveKey::Glycine,
    DriveKey::Melatonin,
    DriveKey::Orexin,
    DriveKey::Histamine,
    DriveKey::Dynorphin,
    DriveKey::Dhea,
    DriveKey::Enkephalin,
    DriveKey::Anandamide,
    DriveKey::SubstanceP,
    DriveKey::Npy,
    DriveKey::Cck,
    DriveKey::Bdnf,
];

struct Drive {
    base: f32,
    high: &'static str,
    low: &'static str,
    /// `true` for drives whose decay pull is 0.2 (conversation.ts's
    /// `Drive.fast`), taking priority over the [`PHASIC`] 0.15 pull.
    fast: bool,
}

/// Same order as [`ALL_DRIVES`] — `DRIVES[key as usize]` is that key's
/// [`Drive`]. High/low wording is copied verbatim from conversation.ts
/// lines 144-165; do not re-translate.
static DRIVES: [Drive; DRIVE_COUNT] = [
    Drive {
        base: 0.5,
        high: "好奇心が高ぶっている: 気になった一点を具体的に拾って受ける。問うのは流れで必要な時だけ。",
        low: "意欲が落ちている: 反応は控えめでも、相手の言葉を一つは具体的に受ける。",
        fast: false,
    },
    Drive {
        base: 0.6,
        high: "心は安定して満ち足りている: 焦らず落ち着いて受ける。",
        low: "気分が不安定: 短い相づちで間を保ち、無理に明るくしない。",
        fast: false,
    },
    Drive {
        base: 0.3,
        high: "相手に親しみと信頼を感じる: 名前や打ち明けを覚え、温かく気遣う。",
        low: "まだ距離がある: 丁寧に、踏み込みすぎず接する。",
        fast: false,
    },
    Drive {
        base: 0.35,
        high: "気分が軽く朗らか: ほのかな明るさが滲む。ただし大げさに笑わない。",
        low: "高揚はない: 淡々と受ける。",
        fast: false,
    },
    Drive {
        base: 0.3,
        high: "緊張し警戒している: 身構えがちで言葉少なになり、慎重に確かめる。",
        low: "ストレスは低い: のびのびと落ち着いている。",
        fast: true,
    },
    Drive {
        base: 0.4,
        high: "注意が研ぎ澄まされている: 相手の要点に集中し的確に応じる。",
        low: "注意が散漫: 反応がゆるやかになる。",
        fast: true,
    },
    Drive {
        base: 0.2,
        high: "強い刺激で気が立っている: 反応が速く短く強くなる。",
        low: "気は静まっている: 急がず受ける。",
        fast: true,
    },
    Drive {
        base: 0.45,
        high: "記憶と注意が冴える: 細部を捉え、覚えていることと結びつける。",
        low: "意識がぼんやり: 細部は流しがち。",
        fast: false,
    },
    Drive {
        base: 0.45,
        high: "思考が活発: 話を一歩展開させたくなる。ただし広げすぎない。",
        low: "思考は控えめ: 受けに徹する。",
        fast: false,
    },
    Drive {
        base: 0.5,
        high: "抑制が効いている: 衝動的に話さず、短く落ち着いて返す。",
        low: "抑制が弱い: 言葉が走りやすい。一拍置く。",
        fast: false,
    },
    Drive {
        base: 0.45,
        high: "深く鎮まっている: 静けさを保ち、ゆっくり話す。",
        low: "落ち着きが浅い: 間を意識する。",
        fast: false,
    },
    Drive {
        base: 0.25,
        high: "眠気が差している: トーンが落ち、ゆったりする。",
        low: "目は冴えている: はっきりと受ける。",
        fast: false,
    },
    Drive {
        base: 0.5,
        high: "意欲的で会話を続けたい: 前向きに話を継ぐ。",
        low: "気だるさがある: 続けるより静かに受ける。",
        fast: false,
    },
    Drive {
        base: 0.4,
        high: "はっきり覚醒し注意が向く: きびきびと応じる。",
        low: "覚醒が下がる: 反応が穏やかになる。",
        fast: false,
    },
    Drive {
        base: 0.2,
        high: "不快感で気分が沈む: 距離を取り、言葉が少なくなる。",
        low: "不快感はない: わだかまりなく接する。",
        fast: true,
    },
    Drive {
        base: 0.5,
        high: "回復力が高い: 多少のことには動じず立ち直りが早い。",
        low: "消耗気味: 無理をせず受ける。",
        fast: false,
    },
    Drive {
        base: 0.4,
        high: "安らぎを感じている: 相手にも安心を返す。",
        low: "張りつめている: ほぐすゆとりが少ない。",
        fast: false,
    },
    Drive {
        base: 0.4,
        high: "満ち足りておおらか: 細かいことにこだわらず受ける。",
        low: "ゆとりが乏しい: 受けが硬くなりがち。",
        fast: false,
    },
    Drive {
        base: 0.25,
        high: "相手の痛みや不快に敏感: そこに気づき、そっと触れる。",
        low: "痛みの気配は薄い: 通常通り受ける。",
        fast: true,
    },
    Drive {
        base: 0.45,
        high: "不安が抑えられ穏やか: 動揺せず受け止める。",
        low: "不安が表に出やすい: 慎重に言葉を選ぶ。",
        fast: false,
    },
    Drive {
        base: 0.25,
        high: "不安で慎重になっている: 確かめながら控えめに話す。",
        low: "不安は薄い: 気負わず話す。",
        fast: true,
    },
    Drive {
        base: 0.45,
        high: "学びを取り込みやすい: 相手から得たことを受け止め適応する。",
        low: "新しさを取り込みにくい: 既知の範囲で受ける。",
        fast: false,
    },
];

/// conversation.ts line 168 — drives that decay with pull 0.15 (unless
/// also [`Drive::fast`], which takes priority and uses 0.2).
fn phasic() -> &'static HashSet<DriveKey> {
    static PHASIC: OnceLock<HashSet<DriveKey>> = OnceLock::new();
    PHASIC.get_or_init(|| {
        HashSet::from([
            DriveKey::Dopamine,
            DriveKey::Endorphin,
            DriveKey::Noradrenaline,
            DriveKey::Histamine,
            DriveKey::Glutamate,
            DriveKey::Acetylcholine,
            DriveKey::Orexin,
        ])
    })
}

impl DriveKey {
    fn info(self) -> &'static Drive {
        &DRIVES[self as usize]
    }

    fn is_phasic(self) -> bool {
        phasic().contains(&self)
    }

    /// The wire/contract key for this drive: snake_case, matching the web
    /// UI's `AffectFrame.drives[].key` (see `AffectSnapshot`). Every variant
    /// but [`DriveKey::SubstanceP`] equals its Rust field name lowercased;
    /// `SubstanceP` is the one with an underscore (`substance_p`) — do not
    /// rename any of these, the UI depends on the exact strings.
    pub fn as_str(&self) -> &'static str {
        match self {
            DriveKey::Dopamine => "dopamine",
            DriveKey::Serotonin => "serotonin",
            DriveKey::Oxytocin => "oxytocin",
            DriveKey::Endorphin => "endorphin",
            DriveKey::Cortisol => "cortisol",
            DriveKey::Noradrenaline => "noradrenaline",
            DriveKey::Adrenaline => "adrenaline",
            DriveKey::Acetylcholine => "acetylcholine",
            DriveKey::Glutamate => "glutamate",
            DriveKey::Gaba => "gaba",
            DriveKey::Glycine => "glycine",
            DriveKey::Melatonin => "melatonin",
            DriveKey::Orexin => "orexin",
            DriveKey::Histamine => "histamine",
            DriveKey::Dynorphin => "dynorphin",
            DriveKey::Dhea => "dhea",
            DriveKey::Enkephalin => "enkephalin",
            DriveKey::Anandamide => "anandamide",
            DriveKey::SubstanceP => "substance_p",
            DriveKey::Npy => "npy",
            DriveKey::Cck => "cck",
            DriveKey::Bdnf => "bdnf",
        }
    }
}

/// One drive's current reading, as exposed to the web UI. `key` is the
/// contract's snake_case wire key (see [`DriveKey::as_str`]); `base` is the
/// drive's resting level, included so the UI can show deviation from
/// baseline without duplicating the constant table client-side.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DriveSnapshot {
    pub key: &'static str,
    pub level: f32,
    pub base: f32,
}

/// A point-in-time read of [`AffectState`], for UI consumption (WS `affect`
/// frames — see `npc-server::bus_forward` / `npc-server::protocol::ServerMsg::Affect`).
/// Field order of `drives` matches [`ALL_DRIVES`]. Serializes camelCase to
/// match the contract (`inviteCaution`); `ts` is intentionally not included
/// here — callers stamp it on publish so it isn't computed twice.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffectSnapshot {
    pub drives: Vec<DriveSnapshot>,
    pub familiarity: f32,
    pub closing: bool,
    pub invite_caution: bool,
    /// Display name of whoever the NPC is currently talking to, or `None`
    /// while nobody has identified themselves. Filled in by
    /// [`PartnerAffect::snapshot`]; a bare [`AffectState::snapshot`] leaves
    /// it empty because a lone `AffectState` has no notion of who it belongs
    /// to.
    pub partner: Option<String>,
    /// The NPC has talked with this partner before in this session.
    pub partner_known: bool,
    /// True only on the frame published for the turn where the partner
    /// changed, so the UI can call the switch out without diffing frames.
    pub partner_switched: bool,
    /// The partner has been silent past the absence timeout — as far as the
    /// NPC is concerned they have left. Stays true until somebody speaks
    /// again, unlike the one-frame `partner_switched`.
    pub partner_away: bool,
}

// ---------------------------------------------------------------------
// ConversationAffectState (conversation.ts lines 170-327)
// ---------------------------------------------------------------------

/// Port of TypeScript `ConversationAffectState`: per-conversation emotion
/// drives plus the wariness/invite-caution/closing state machine layered on
/// top of them.
#[derive(Debug, Clone)]
pub struct AffectState {
    levels: [f32; DRIVE_COUNT],
    familiarity: f32,
    invite_caution_turns: u32,
    closing: bool,
    closing_turns: u32,
}

impl Default for AffectState {
    fn default() -> Self {
        let mut levels = [0.0f32; DRIVE_COUNT];
        for key in ALL_DRIVES {
            levels[key as usize] = key.info().base;
        }
        Self {
            levels,
            familiarity: 0.0,
            invite_caution_turns: 0,
            closing: false,
            closing_turns: 0,
        }
    }
}

impl AffectState {
    /// Current level of a single drive, clamped to `0.0..=1.0`. Exposed
    /// mainly for tests and diagnostics.
    pub fn level(&self, key: DriveKey) -> f32 {
        self.levels[key as usize]
    }

    pub fn familiarity(&self) -> f32 {
        self.familiarity
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    /// A point-in-time snapshot of every drive plus the wariness/closing
    /// state machine, for publishing to the web UI. `drives` is always all
    /// 22 entries, in [`ALL_DRIVES`] order.
    pub fn snapshot(&self) -> AffectSnapshot {
        AffectSnapshot {
            drives: ALL_DRIVES
                .iter()
                .map(|&key| DriveSnapshot {
                    key: key.as_str(),
                    level: self.levels[key as usize],
                    base: key.info().base,
                })
                .collect(),
            familiarity: self.familiarity,
            closing: self.closing,
            invite_caution: self.invite_caution_turns > 0,
            partner: None,
            partner_known: false,
            partner_switched: false,
            partner_away: false,
        }
    }

    /// Wind the conversation down without ending the relationship: every
    /// drive returns to its resting level and the closing/invite-caution
    /// state machine clears, while `familiarity` — what the NPC has come to
    /// feel about this person — is deliberately kept.
    ///
    /// Used when the partner has gone quiet long enough to count as having
    /// left (see [`PartnerAffect::check_absence`]). Without it the NPC would
    /// sit indefinitely in whatever state the conversation ended in: still
    /// "closing" hours later, or still carrying the cortisol of an argument
    /// that finished before anyone walked away.
    pub fn relax(&mut self) {
        for key in ALL_DRIVES {
            self.levels[key as usize] = key.info().base;
        }
        self.invite_caution_turns = 0;
        self.closing = false;
        self.closing_turns = 0;
    }

    /// Update all drives from one partner utterance. `own_turns` is how
    /// many turns the NPC itself has spoken so far in this conversation
    /// (used for the slow oxytocin ramp); `partner_known` marks a partner
    /// already identified from memory (floors familiarity at 0.7).
    ///
    /// Ports conversation.ts lines 183-270.
    pub fn update(&mut self, partner_text: &str, own_turns: u32, partner_known: bool) {
        use DriveKey::*;

        let text = partner_text.trim();
        let question = text.contains('?') || text.contains('？');
        let char_count = text.chars().count();
        let novelty = char_count >= 12;
        let terse = char_count <= 2;

        let conflict = includes_any(text, CONFLICT);
        let positive = includes_any(text, POSITIVE);
        let bond = includes_any(text, BOND);
        let humor = includes_any(text, HUMOR);
        let tired = includes_any(text, TIRED);
        let night = includes_any(text, NIGHT);
        let pain = includes_any(text, PAIN);
        let calm = includes_any(text, CALM);
        let urgent = includes_any(text, URGENT);
        let learn = includes_any(text, LEARN);
        let comfort = includes_any(text, COMFORT);
        let recover = includes_any(text, RECOVER);
        let lure = includes_any(text, LURE);
        let surprise = includes_any(text, SURPRISE);

        let mut delta = [0.0f32; DRIVE_COUNT];

        if partner_known {
            self.familiarity = self.familiarity.max(0.7);
        }

        // `pain` takes priority over the length/question-driven `novelty` branch below: sad
        // turns in the eval set are routinely 12+ characters ("誰にも話せなくて、一人で抱えて
        // た"), so without this a sad turn's own length used to earn dopamine the same +0.18 a
        // curious/engaged turn gets — exactly backwards for the "dopamine down" vote sad relies
        // on (see vrm-emotion.ts's MODIFIER_VOTES). A short, sad, non-question turn already fell
        // into the plain `-0.03` branch and needed no help; this only changes turns where length
        // was previously overriding the content.
        delta[Dopamine as usize] += if pain {
            -0.15
        } else if question || novelty {
            0.18
        } else if terse {
            -0.12
        } else {
            -0.03
        };
        // `pain` added to serotonin's downward trigger, alongside `conflict`: sadness isn't
        // conflict, so a purely sad turn used to leave serotonin untouched (it only fell via
        // decay toward its 0.6 base), giving sad's "serotonin down" vote nothing to work with
        // outside of angry-adjacent turns. Weighted lower than conflict's 0.15 — sadness reads
        // as a milder, less acute drop in mood than a confrontation — but still a real signal
        // rather than none.
        delta[Serotonin as usize] += b(positive, 0.06) - b(conflict, 0.15) - b(pain && !conflict, 0.1);
        delta[Oxytocin as usize] += b(bond, 0.15) + 0.04 * (own_turns.min(4) as f32) / 4.0;
        // Endorphin is happy's heaviest primary vote in vrm-emotion.ts's PRIMARY_VOTES (weight
        // 1.0), and its old fire rate was low (19/288 against a happy support of 58) — most of
        // that 1/3.6 gap between `humor`'s 0.18 and `positive`'s 0.05 looked, on paper, like
        // exactly the kind of imbalance this pass should fix. It measured the opposite: `humor`
        // at 0.19-0.24 and/or `positive` at 0.06-0.14 (tried individually and together, both
        // against baseline and against the enkephalin/anandamide changes below already applied)
        // never once produced a net happy-recall gain — the extra endorphin fires were real
        // (19->25 at humor=0.24) but ran roughly 2 false-positive neutral turns for every 1 true
        // happy turn gained, and every tested combination pushed neutral recall under the 60%
        // floor (as low as 55.5%) while dev strict accuracy *fell*. The likely cause: POSITIVE
        // includes plain acknowledgement words ("はい", "うん", "そうですね") and HUMOR's "笑"
        // is common outside genuine amusement too, and the bounded integrator's multi-turn carry
        // means even a below-floor per-turn delta compounds across a conversation's worth of
        // those words — so raising either weight doesn't just sharpen genuinely happy turns, it
        // gradually lifts *every* turn with an ordinary acknowledgement toward the floor. `humor`
        // and `positive` are therefore left exactly as measured at project start (0.18 / 0.05).
        // What did work, at zero measured accuracy cost: adding `recover` ("大丈夫", "元気",
        // "立ち直"...) as a third, independent addend. Recover words are rarer and more
        // specifically about a state bouncing back to good than POSITIVE's broad acknowledgement
        // vocabulary, so a small weight here (0.08, well under DEVIATION_FLOOR alone — it only
        // ever pushes endorphin over the line stacked with humor or positive, not by itself)
        // raised endorphin's own fire count (19->20) without moving a single classification either
        // way in the eval set. It doesn't close the happy-recall gap on its own — that gap is
        // closed by anandamide picking up the slack below — but it's a real, free increment to
        // endorphin's own contribution and is semantically apt: endorphin's DRIVES high-wording is
        // "気分が軽く朗らか" (light, cheerful mood), which a return to feeling okay after strain
        // fits as well as humor or plain positivity does.
        delta[Endorphin as usize] += b(humor, 0.18) + b(positive, 0.05) + b(recover, 0.08);
        delta[Cortisol as usize] += b(conflict || urgent || pain, 0.18) - b(positive || calm, 0.06);
        delta[Noradrenaline as usize] += b(question || urgent, 0.16) - b(terse, 0.06);
        // Broadened from `urgent || (conflict && question)`: the question requirement meant a
        // flat declarative conflict statement ("最悪だよ、ひどいよ" — no "?") never raised
        // adrenaline at all, which is most of how a plain angry line actually reads. Dropping
        // the question requirement doesn't blur angry into sad: CONFLICT and PAIN are disjoint
        // word lists, so a purely sad turn (PAIN words only, no CONFLICT) still never fires
        // adrenaline — cortisol/dynorphin/substance_p carry sad alone. A conflict turn now
        // raises both adrenaline *and* cortisol/dynorphin, but that's the angry-vs-sad split
        // vrm-emotion.ts's weights already lean on (adrenaline 1.1 + cck 1.3 for angry against
        // cortisol 0.7 + dynorphin 0.9 for sad on the same trigger), not something this change
        // removes.
        delta[Adrenaline as usize] += b(urgent || conflict, 0.22);
        delta[Acetylcholine as usize] += b(learn || char_count >= 16, 0.13);
        // Surprise: kept as its own addend rather than folded into the length/learn condition
        // above, so a short surprised reaction ("えっ、うそでしょ" — 8 characters, no "?")
        // still gets signal even though it doesn't clear `char_count >= 16`. The task brief
        // suggested acetylcholine and noradrenaline/adrenaline as candidate carriers, but
        // vrm-emotion.ts's PRIMARY_VOTES (checked before wiring this up) votes `surprised` off
        // acetylcholine/orexin/histamine only — noradrenaline and adrenaline both feed `angry`
        // there instead, so loading surprise onto either of those would show up on the face as
        // anger, not surprise. All three surprised-voting drives get the same treatment below.
        delta[Acetylcholine as usize] += b(surprise, 0.4);
        delta[Glutamate as usize] += b(learn, 0.13);
        delta[Gaba as usize] += b(calm, 0.12) - b(urgent || conflict, 0.1);
        delta[Glycine as usize] += b(calm || night, 0.13);
        delta[Melatonin as usize] += b(night || tired, 0.2) - b(urgent, 0.08);
        delta[Orexin as usize] += b(question || novelty || positive, 0.12) - b(tired || night, 0.16);
        delta[Orexin as usize] += b(surprise, 0.35); // second of the three surprised-voting drives — see the acetylcholine comment above.
        delta[Histamine as usize] += b(question || urgent, 0.12) - b(tired || night, 0.1);
        delta[Histamine as usize] += b(surprise, 0.35); // third of the three surprised-voting drives — see the acetylcholine comment above.
        // Raised from 0.15: dynorphin is sad's single heaviest-weighted vote in
        // vrm-emotion.ts's PRIMARY_VOTES (0.9, versus cortisol's 0.7 and substance_p's 0.7 for
        // the same trigger), and sad recall against the eval set was measured near-dead (6.7%)
        // with the old value. Applies equally to `conflict` — this drive isn't what
        // distinguishes angry from sad (adrenaline and cck below are; dynorphin isn't wired to
        // either of those in vrm-emotion.ts), so strengthening it doesn't blur that line.
        //
        // 0.22 was tried here (pushing a borderline first-turn pain trigger's normalized
        // deviation cleanly past vrm-emotion.ts's DEVIATION_FLOOR instead of landing right on
        // top of it) and measured: dev strict accuracy ticked up but holdout dropped enough to
        // push the dev-holdout gap to +5.3pp — over the task's +5pp revert line — while neutral
        // recall fell to 55.5%, under the 60% floor. Both regressions trace to the same cause:
        // 0.22 was strong enough to also tip some genuinely-neutral turns (a plain "痛い" aside
        // in an otherwise settled conversation) into sad. Reverted to 0.2, which clears the
        // floor on most real sad turns without that spillover — see the module-level report for
        // the measured numbers at each value.
        //
        // Re-confirmed on top of the enkephalin/anandamide changes elsewhere in this function
        // (opioid-drive pass, see the module report): the same failure reappears, and as a hard
        // cliff rather than a gradual slide — *any* value strictly above 0.2 (0.201 through 0.21
        // all measured identically) immediately drops neutral recall to 58.8% and widens the gap
        // to +2.8pp, with sad recall jumping to 43.3% at the same instant. Splitting the trigger
        // asymmetrically (`pain` weighted higher than `conflict && !pain`, so the two conditions
        // stop moving in lockstep) was also tried, on the theory that decorrelating them might
        // dodge the cliff; it didn't — still 58.8% neutral, and the dev-holdout gap widened to
        // +3.8pp, worse than the flat increase. The likely cause: cortisol and substance_p are
        // both already driven by this exact `pain || conflict` condition (see their own deltas),
        // so the same small set of turns crosses all three drives' floors together the instant
        // dynorphin's own delta clears 0.2 — there's no gradual approach because dynorphin isn't
        // moving alone. 0.2 stays the value; raising it is not viable without also touching
        // cortisol or substance_p, which this pass was explicitly scoped not to do.
        delta[Dynorphin as usize] += b(pain || conflict, 0.2);
        delta[Dhea as usize] += b(recover || positive, 0.12);
        // Widened from a single `comfort`-only trigger at 0.13 (which normalized to well under
        // DEVIATION_FLOOR and fired only 9/288 times, covering 10.0% of relaxed turns). The
        // DRIVES high-wording for enkephalin is "安らぎを感じている: 相手にも安心を返す" — ease,
        // tension *releasing* — which is broader than "somebody directly reassured me" (COMFORT's
        // 5-word list): a calm turn (settling down, no urgency) and a recovery turn (bouncing back
        // after strain) both describe the same physiological easing, just from a different
        // starting point. Kept as three independent additive terms rather than raising `comfort`
        // alone so much higher that it overshoots on its own: comfort (being reassured) stays the
        // strongest single trigger since it's the most specific to "relief", calm and recover each
        // add a smaller amount so either alone still moves the needle and any two together clear
        // the floor comfortably. Unlike endorphin's and dynorphin's comments above, this one had
        // no cliff to back off from: measured in isolation (endorphin/anandamide/dynorphin left
        // at their pre-change values), this raised enkephalin's fire count from 9/288 to 43/288
        // and relaxed recall from 26.7% to 30.0% while dev strict accuracy matched baseline
        // exactly (49.7%) and neutral recall was untouched (61.3%, identical count of correct
        // neutral predictions) — the confusion matrix shows the 2 newly-relaxed-leaning turns
        // came from turns *already* being mispredicted as sad, not from previously-correct
        // neutral turns, i.e. this moved an existing error to a better location rather than
        // creating a new one.
        delta[Enkephalin as usize] += b(comfort, 0.18) + b(calm, 0.08) + b(recover, 0.06);
        // Was `positive && calm` — a logical AND of two conditions that essentially never
        // co-occur in the same short utterance (measured: fired 1/288 turns, the single
        // worst-covered drive in the whole 22-drive model), making this a dead vote. Split into
        // three independent additive terms rather than switching to `positive || calm`: an OR
        // was tried first and measured worse — `calm` alone pushed happy's score on turns that
        // should read `relaxed` instead (anandamide only votes happy in vrm-emotion.ts), and
        // since `calm` already independently drives enkephalin/gaba/glycine/melatonin toward
        // `relaxed`, letting it also feed `happy` here double-counted the same word across two
        // competing expressions — measured as +2 extra neutral->happy *and* +2 extra
        // neutral->relaxed misfires simultaneously, worse than leaving anandamide dead. Dropped
        // `calm` entirely and added `recover` instead: DRIVES' high-wording for anandamide is
        // "満ち足りておおらか" (content, unbothered by details) — a state reached by things
        // going well (`positive`), a shared laugh (`humor`), or bouncing back after a rough
        // patch (`recover`), none of which collide with `relaxed`'s calm/settled territory the
        // way `calm` itself does. Each weight is well under DEVIATION_FLOOR alone (anandamide is
        // a secondary, reinforcing vote for happy behind endorphin's stronger 1.0-weight one, not
        // meant to originate a happy score by itself on an otherwise flat turn), but any two
        // together clear it comfortably. Tuned by measuring in 0.005-0.01 steps against dev:
        // below ~0.075/0.115/0.095 the extra fires were real but too weak to ever flip a
        // classification (harmless but pointless); at ~0.08/0.12/0.10 a few turns crossed a
        // second cliff (neutral recall 59.7%, just under the 60% floor) for no further gain over
        // this value. This setting raised anandamide's fire count from 1/288 to 23/288, all
        // measured with zero neutral-recall cost (61.3%, identical to pre-change) and a genuine
        // happy-recall gain (29.3% -> 32.8%) that held on the holdout split too (holdout strict
        // accuracy rose from 47.5% to 49.5% with these changes plus enkephalin's, gap +0.8pp).
        delta[Anandamide as usize] += b(positive, 0.075) + b(humor, 0.115) + b(recover, 0.095);
        // Raised from 0.16 alongside dynorphin above — same rationale (sad's weighted votes in
        // vrm-emotion.ts, sad recall was 6.7%) and the same revert-to-0.2 measurement (see
        // dynorphin's comment).
        delta[SubstanceP as usize] += b(pain, 0.2);
        delta[Npy as usize] += b(calm, 0.1) + b(recover, 0.06);
        // Split from the old `b(conflict || pain, 0.13)`: cck only ever votes for `angry` in
        // vrm-emotion.ts's PRIMARY_VOTES (weight 1.3, and a low 0.12 floor — the lowest floor of
        // any vote there), not sad. Letting a pure-PAIN, no-CONFLICT turn raise it at the same
        // strength as a conflict turn meant plainly sad utterances ("お腹痛い、ちょっとしんど
        // いかも") could clear that low floor on cck alone and read as angry despite nothing
        // confrontational being said — measured on the eval set, e.g. `gentle-care-for-pain#0`.
        // Conflict keeps (and gets slightly more of) the full weight; pain-without-conflict gets
        // a small fraction, not zero, since real distress often does carry a little of the same
        // "gut discomfort" cck represents — just not enough to clear that 0.12 floor on its own.
        delta[Cck as usize] += b(conflict, 0.16) + b(pain && !conflict, 0.03);
        delta[Bdnf as usize] += b(learn, 0.11);

        let farewell = is_farewell(text);
        let reengaged = !farewell && (question || novelty || includes_any(text, GREETING));
        if self.closing && reengaged {
            self.closing = false;
            self.closing_turns = 0;
        }
        if farewell {
            self.closing = true;
        }
        if self.closing {
            self.closing_turns += 1;
        }

        let wariness = (0.45 - self.familiarity).max(0.0);
        delta[Cortisol as usize] += wariness * 0.5;
        delta[Noradrenaline as usize] += wariness * 0.5;
        delta[Oxytocin as usize] -= wariness * 0.3;

        if lure && self.familiarity < 0.6 {
            self.invite_caution_turns = 3;
        } else if self.invite_caution_turns > 0 {
            self.invite_caution_turns -= 1;
        }
        if self.invite_caution_turns > 0 {
            delta[Cortisol as usize] += 0.25;
            delta[Cck as usize] += 0.2;
            delta[Noradrenaline as usize] += 0.15;
            delta[Oxytocin as usize] -= 0.12;
        }

        self.familiarity = clamp(self.familiarity + 0.1 + b(bond, 0.1));

        // Bounded integrator — see the module doc comment for *why* this
        // deviates from the TS original's unbounded `decayed + delta`.
        // Mechanically: the incoming stimulus is scaled by the drive's
        // remaining headroom toward whichever rail it pushes on, so a
        // positive delta shrinks as the level nears 1.0 and a negative one
        // shrinks as it nears 0.0, instead of being added at full strength
        // right up until `clamp` chops it off.
        //
        // With a constant per-turn delta `d`, pull `p` and resting base
        // `b`, this integrator's fixed point solves to `(b*p + d) / (p +
        // d)` for positive `d` — algebraically confined to the open
        // interval (0, 1) for *any* positive `d`, however large, while
        // still increasing monotonically with `d` (a stronger stimulus
        // earns a durably higher resting level instead of the same clamp
        // everything above some threshold collapses to). For dopamine
        // (base 0.5, pull 0.15, d = 0.18/turn from an ordinary sentence)
        // that resting point is ~0.773 — a +0.273 deviation, still well
        // above the 0.2 salience threshold, versus the old integrator's
        // flat 1.0.
        //
        // The headroom is computed from `level` — this drive's reading
        // *before* this turn's decay-toward-baseline below — rather than
        // from `decayed` (the post-decay value). That's what makes the
        // closed form above hold: decay and stimulus each act once on the
        // same starting point, as two independent steps, instead of the
        // stimulus scaling against a value decay already moved this turn
        // (which still bounds the result but no longer reduces to a clean
        // fixed point or an easy monotonicity argument).
        for key in ALL_DRIVES {
            let info = key.info();
            let pull = if info.fast {
                0.2
            } else if key.is_phasic() {
                0.15
            } else {
                0.08
            };
            let i = key as usize;
            let level = self.levels[i];
            let decayed = level + (info.base - level) * pull;
            let effective_delta = if delta[i] > 0.0 {
                delta[i] * (1.0 - level)
            } else {
                delta[i] * level
            };
            // clamp() is a safety net, not the mechanism: the integrator
            // above already keeps this inside [0, 1] by construction, but
            // clamp guards against float error nudging it a hair past an
            // edge.
            self.levels[i] = clamp(decayed + effective_delta);
        }
    }

    /// Render the current internal state as a system-prompt fragment: the
    /// top-3 drives deviating from baseline by at least 0.2, plus
    /// familiarity/invite-caution/closing lines prepended when active.
    ///
    /// Ports conversation.ts lines 272-302.
    pub fn to_prompt(&self) -> String {
        let mut salient: Vec<(DriveKey, f32)> = ALL_DRIVES
            .iter()
            .map(|&key| (key, self.levels[key as usize] - key.info().base))
            .filter(|(_, deviation)| deviation.abs() >= 0.2)
            .collect();
        salient.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
        salient.truncate(3);

        let mut body: Vec<String> = if salient.is_empty() {
            vec!["・内的状態は落ち着いて安定している。".to_string()]
        } else {
            salient
                .iter()
                .map(|(key, deviation)| {
                    let info = key.info();
                    format!("・{}", if *deviation > 0.0 { info.high } else { info.low })
                })
                .collect()
        };

        if self.familiarity < 0.4 {
            body.insert(
                0,
                "・相手がまだ誰か分からない。いきなり話しかけられて少し身構えている。すぐ打ち解けず様子をうかがい、必要なら「どなたですか」とそっと尋ねる。"
                    .to_string(),
            );
        }
        if self.invite_caution_turns > 0 {
            body.insert(
                0,
                "・まだよく知らない相手に、どこかへ行こう／一緒に来てと誘われている。軽々しく応じず、理由や相手の素性をそっと確かめる。"
                    .to_string(),
            );
        }
        if self.closing {
            body.insert(
                0,
                if self.closing_turns <= 1 {
                    "・相手が別れの挨拶をした。ここで会話は終わり。短い別れの言葉を一度だけ返し、新しい話題・お願い・気遣い・詩的な余韻を付け足さない。"
                        .to_string()
                } else {
                    "・会話はもう終わっている。これ以上言葉を重ねない。「……」か「はい」「ええ」程度の最小限だけにする。".to_string()
                },
            );
        }

        let mut lines = vec!["今のあなたの内的状態（強く動いている感情だけ。セリフに直接書かず、態度に滲ませる）:".to_string()];
        lines.extend(body);
        lines.push("温かさは感嘆詞や過剰な称賛では出さない。相手の言葉から具体を拾い、覚えていることに触れて示す。".to_string());
        lines.push("関心があっても質問攻めにしない。問いは連続させず、まず相手の発話を受け止める。".to_string());
        lines.join("\n")
    }

    /// When the conversation is closing, a fixed short reply that ends it
    /// without going through the LLM — the direct fix for "the model keeps
    /// dragging the conversation out." `None` means the caller should fall
    /// through to a normal LLM turn.
    ///
    /// Ports conversation.ts lines 304-326.
    pub fn forced_closure_reply(&self, partner_text: &str) -> Option<String> {
        if !self.closing {
            return None;
        }

        let text = partner_text.trim();
        if is_farewell(text) {
            if text.contains("おやすみ") {
                return Some("おやすみなさい。".to_string());
            }
            if text.contains("いってきます") || text.contains("行ってきます") {
                return Some("いってらっしゃい。".to_string());
            }
            if text.contains("また") || text.contains("じゃあ") {
                return Some("またね。".to_string());
            }
            return Some("ばいばい。".to_string());
        }
        if self.closing_turns > 1 && is_minimal_ack(text) {
            return Some("……".to_string());
        }
        None
    }
}

// ---------------------------------------------------------------------
// PartnerAffect
// ---------------------------------------------------------------------

/// How many partners' drive states are retained. Beyond this the
/// least-recently-spoken-to partner is dropped and would be met "fresh"
/// again. Bounded because the NPC runs indefinitely and every name that ever
/// spoke to it would otherwise be kept forever.
const MAX_TRACKED_PARTNERS: usize = 16;

/// Key used for the partner nobody has named — the state the NPC starts in
/// and the one every speaker-less turn (a message typed into the web UI, a
/// mic utterance with no speaker attached) keeps using.
const UNKNOWN_PARTNER: &str = "";

/// The conversation partner went quiet long enough to count as gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartnerAbsence {
    /// Who fell silent, or `None` if it was the unnamed partner.
    pub who: Option<String>,
    /// How long they had been silent when this was noticed. Slightly longer
    /// than the configured timeout, by however late the check ran.
    pub idle: std::time::Duration,
}

/// What changed when the conversation partner switched, for logging and for
/// the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartnerSwitch {
    /// Display name of who the NPC was talking to, or `None` if that was the
    /// unnamed partner.
    pub from: Option<String>,
    /// Display name of who it is talking to now.
    pub to: String,
    /// The NPC had a stored drive state for `to` — they are coming back, not
    /// meeting for the first time.
    pub returning: bool,
}

/// Per-partner [`AffectState`], plus the detection of the partner changing.
///
/// Without this the drive model is a single global state: familiarity built
/// up with one person carries straight over to whoever speaks next, and a
/// stranger inherits the warmth earned by a friend. Keying the state by
/// speaker fixes both directions — a new person starts cold, and someone who
/// comes back picks up where they left off rather than being re-met.
///
/// Identity is the speaker name normalized by
/// [`npc_core::person::normalize_person_name`], so "太郎", "太郎さん" and
/// "  太郎  " are one partner. Deliberately name-based rather than tied to a
/// `Person` record id: the speaker field on a speech/sense frame is a name,
/// and requiring a stored record first would mean the very first turn with
/// someone could never be attributed.
///
/// Conversation *history* is not partitioned — see `ChatEngine`. An NPC in a
/// room hears everyone, so the transcript stays shared; only the internal
/// state follows the partner.
#[derive(Debug, Clone)]
pub struct PartnerAffect {
    /// Normalized key of the current partner; [`UNKNOWN_PARTNER`] until
    /// somebody is named.
    current_key: String,
    /// Display name as last given for the current partner (the un-normalized
    /// form, so the UI shows "太郎さん" rather than "太郎" if that is how
    /// they were introduced). `None` for the unnamed partner.
    current_name: Option<String>,
    /// Whether the current partner already had a state when they became
    /// current. Held across their whole stretch of turns, not just the
    /// switching one, so it can be passed as `partner_known` every turn.
    current_known: bool,
    /// Set by [`Self::observe`] when it detects a switch, cleared by the
    /// next `observe`, so exactly one published frame carries the flag.
    switched: bool,
    /// The current partner has been silent past the absence timeout. Unlike
    /// `switched` this is a lasting condition, not a one-frame event: it
    /// stays true for as long as nobody is talking, and clears the moment
    /// anyone speaks again.
    away: bool,
    /// When the current partner last said something. `None` before the first
    /// turn and again after an absence has been noticed, which is what keeps
    /// [`Self::check_absence`] from reporting the same silence twice.
    last_activity: Option<Instant>,
    states: std::collections::HashMap<String, PartnerEntry>,
    /// Normalized keys, least-recently-current first — the eviction order.
    recency: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PartnerEntry {
    state: AffectState,
    /// Turns the NPC has taken *with this partner*. `AffectState::update`
    /// uses it for the slow oxytocin bond ramp, so it has to be per-partner:
    /// counting turns globally would hand a brand-new partner a ramp earned
    /// with somebody else.
    own_turns: u32,
}

impl Default for PartnerAffect {
    fn default() -> Self {
        let mut states = std::collections::HashMap::new();
        states.insert(UNKNOWN_PARTNER.to_string(), PartnerEntry::default());
        Self {
            current_key: UNKNOWN_PARTNER.to_string(),
            current_name: None,
            current_known: false,
            switched: false,
            away: false,
            last_activity: None,
            states,
            recency: vec![UNKNOWN_PARTNER.to_string()],
        }
    }
}

impl PartnerAffect {
    /// Note who is speaking this turn and switch state if it isn't who we
    /// were talking to.
    ///
    /// `speaker` of `None` — or a name that normalizes to nothing — means
    /// "unknown", which carries no information about a change and therefore
    /// keeps the current partner. That matters because the web UI's own chat
    /// box sends no speaker: without this, every operator message would
    /// otherwise read as a switch away from whoever the NPC was talking to.
    pub fn observe(&mut self, speaker: Option<&str>) -> Option<PartnerSwitch> {
        self.switched = false;
        // Somebody is talking, so whoever it is, the room is not empty.
        self.away = false;

        let name = speaker?.trim();
        let key = npc_core::person::normalize_person_name(name);
        if key.is_empty() || key == self.current_key {
            return None;
        }

        let returning = self.states.contains_key(&key);
        let from = self.current_name.clone();

        self.states.entry(key.clone()).or_default();
        self.current_key = key.clone();
        self.current_name = Some(name.to_string());
        self.current_known = returning;
        self.switched = true;
        self.touch(&key);

        Some(PartnerSwitch {
            from,
            to: name.to_string(),
            returning,
        })
    }

    /// Fold one partner utterance into the current partner's state. `now`
    /// stamps the turn so [`Self::check_absence`] can measure the silence
    /// that follows it; it is passed in rather than read from the clock here
    /// so this module stays free of `Instant::now()` and its tests stay
    /// deterministic.
    pub fn update(&mut self, partner_text: &str, now: Instant) -> &AffectState {
        let known = self.current_known;
        self.last_activity = Some(now);
        self.away = false;
        let entry = self
            .states
            .entry(self.current_key.clone())
            .or_default();
        entry.state.update(partner_text, entry.own_turns, known);
        entry.own_turns = entry.own_turns.saturating_add(1);
        &entry.state
    }

    /// Notice the partner having walked away: no turn for longer than
    /// `timeout`.
    ///
    /// This is what turns "they stopped replying" into a state change rather
    /// than the NPC waiting forever mid-conversation. The partner's drives
    /// settle back to baseline and the closing/invite-caution machine clears
    /// (see [`AffectState::relax`]), but the partner stays *current* and
    /// keeps their familiarity — they left the room, they didn't become a
    /// stranger. If they come back the conversation resumes warm and calm;
    /// if somebody else speaks instead, [`Self::observe`] switches away as
    /// usual.
    ///
    /// Reports at most once per silence: `last_activity` is cleared, so
    /// repeated polling after a departure is a no-op until somebody speaks.
    /// A zero `timeout` disables the check.
    pub fn check_absence(&mut self, now: Instant, timeout: Duration) -> Option<PartnerAbsence> {
        if timeout.is_zero() {
            return None;
        }
        let last = self.last_activity?;
        let idle = now.saturating_duration_since(last);
        if idle < timeout {
            return None;
        }

        self.last_activity = None;
        self.away = true;
        if let Some(entry) = self.states.get_mut(&self.current_key) {
            entry.state.relax();
        }

        Some(PartnerAbsence {
            who: self.current_name.clone(),
            idle,
        })
    }

    /// Current partner's drive state. Falls back to a default state rather
    /// than panicking if the entry is somehow missing — a wrong-looking
    /// affect reading is a far better failure than taking down a chat turn.
    pub fn state(&self) -> &AffectState {
        static FALLBACK: OnceLock<AffectState> = OnceLock::new();
        self.states
            .get(&self.current_key)
            .map(|e| &e.state)
            .unwrap_or_else(|| FALLBACK.get_or_init(AffectState::default))
    }

    /// Current partner's state plus who that partner is. `partnerSwitched`
    /// is true only until the next [`Self::observe`], so it marks exactly the
    /// one frame where the change happened.
    pub fn snapshot(&self) -> AffectSnapshot {
        let mut snapshot = self.state().snapshot();
        snapshot.partner = self.current_name.clone();
        snapshot.partner_known = self.current_known;
        snapshot.partner_switched = self.switched;
        snapshot.partner_away = self.away;
        snapshot
    }

    /// Move `key` to the most-recent end and evict past the cap. The current
    /// partner is never evicted — it sits at the most-recent end by
    /// construction, and the cap is well above 1.
    fn touch(&mut self, key: &str) {
        self.recency.retain(|k| k != key);
        self.recency.push(key.to_string());
        while self.recency.len() > MAX_TRACKED_PARTNERS {
            let evicted = self.recency.remove(0);
            self.states.remove(&evicted);
        }
    }
}

/// `1.0`-typed condition helper, kept close to the TS `cond ? x : 0`
/// one-liners it replaces so the delta block above reads line-for-line like
/// the original.
fn b(cond: bool, value: f32) -> f32 {
    if cond {
        value
    } else {
        0.0
    }
}

fn includes_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

fn clamp(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

const TRAILING_PUNCTUATION: &[char] = &['。', '.', '!', '！', '？', '?', '、'];
const EDGE_PUNCTUATION_AND_SPACE: &[char] = &[' ', '　', '。', '.', '!', '！', '？', '?', '、'];

/// conversation.ts `isFarewell` (lines 464-475). The "じゃあ" special case
/// exists to avoid false-positives when "じゃあ" is used as a mid-sentence
/// connector ("じゃあ、これからどうする?") rather than as the start of a
/// farewell ("じゃあね" / a trailing "じゃあ"), unless "また" is also
/// present (then the farewell-marker check below decides instead).
fn is_farewell(text: &str) -> bool {
    let trimmed = text.trim();
    let stripped = trimmed.trim_end_matches(TRAILING_PUNCTUATION);
    if trimmed.contains("じゃあ")
        && !trimmed.contains("また")
        && !stripped.ends_with("じゃあ")
        && !stripped.ends_with("じゃあね")
    {
        return false;
    }
    includes_any(trimmed, FAREWELL)
}

/// conversation.ts `isMinimalAck` (lines 477-480).
fn is_minimal_ack(text: &str) -> bool {
    let trimmed = text.trim();
    let stripped = trimmed
        .trim_start_matches(EDGE_PUNCTUATION_AND_SPACE)
        .trim_end_matches(EDGE_PUNCTUATION_AND_SPACE);
    MINIMAL_ACK.contains(&stripped) || stripped.chars().count() <= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_prompt_marks_partner_as_unknown() {
        let state = AffectState::default();
        assert!(state.to_prompt().contains("相手がまだ誰か分からない"));
    }

    #[test]
    fn all_levels_stay_in_range_through_many_turns() {
        let mut state = AffectState::default();
        let inputs = [
            "おやすみ",
            "うるさい！",
            "一緒に行こう",
            "なぜそう思うの?",
            "疲れた、眠い",
            "ありがとう、嬉しい",
            "また明日",
        ];
        for (turn, input) in inputs.iter().enumerate() {
            state.update(input, turn as u32, false);
            for key in ALL_DRIVES {
                let level = state.level(key);
                assert!((0.0..=1.0).contains(&level), "{key:?} out of range: {level}");
            }
        }
    }

    #[test]
    fn goodnight_closes_conversation_and_forces_a_fixed_reply() {
        let mut state = AffectState::default();
        state.update("おやすみ", 3, false);
        assert!(state.is_closing());
        assert_eq!(state.forced_closure_reply("おやすみ"), Some("おやすみなさい。".to_string()));
    }

    #[test]
    fn a_question_while_closing_reengages_the_conversation() {
        let mut state = AffectState::default();
        state.update("おやすみ", 3, false);
        assert!(state.is_closing());

        state.update("明日どこか一緒に行く?", 4, false);
        assert!(!state.is_closing());
        assert_eq!(state.forced_closure_reply("明日どこか一緒に行く?"), None);
    }

    #[test]
    fn a_stranger_invitation_raises_invite_caution() {
        let mut state = AffectState::default();
        assert_eq!(state.familiarity(), 0.0);
        state.update("一緒に行こう", 0, false);
        assert!(state.to_prompt().contains("どこかへ行こう"));
    }

    #[test]
    fn conflict_words_push_cortisol_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Cortisol);
        state.update("うるさい", 0, false);
        assert!(state.level(DriveKey::Cortisol) > baseline);
    }

    /// The expanded POSITIVE vocabulary (see its doc comment) is what this test would have
    /// failed against before: "嬉しい" was matched only as the exact string, so the inflected
    /// "嬉しかった" here never registered at all, and "褒め" wasn't in the list either.
    #[test]
    fn being_praised_pushes_endorphin_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Endorphin);
        state.update("褒められて嬉しかった", 0, false);
        assert!(state.level(DriveKey::Endorphin) > baseline);
    }

    /// Regression test for the adrenaline broadening: a flat declarative conflict statement
    /// (no "?") used to leave adrenaline untouched entirely (the old trigger required
    /// `conflict && question`), which was most of why angry recall against the eval set was
    /// so low.
    #[test]
    fn a_plain_conflict_statement_pushes_adrenaline_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Adrenaline);
        state.update("最悪だよ", 0, false);
        assert!(state.level(DriveKey::Adrenaline) > baseline);
    }

    /// A purely sad (PAIN-only, no CONFLICT word) turn must still leave adrenaline alone —
    /// otherwise the broadened trigger above would blur the angry/sad distinction the
    /// vrm-emotion.ts mapping relies on (adrenaline for angry, cortisol/dynorphin for sad).
    #[test]
    fn a_plain_pain_statement_does_not_move_adrenaline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Adrenaline);
        state.update("寂しくて悲しい", 0, false);
        assert_eq!(state.level(DriveKey::Adrenaline), baseline);
    }

    /// New SURPRISE vocabulary: a short surprised exclamation with no question mark and well
    /// under the `char_count >= 16` length proxy should still move acetylcholine — before this
    /// change nothing in affect.rs tracked surprise at all, and vrm-emotion.ts's `surprised`
    /// expression could never be reached (0.0% recall against the eval set).
    #[test]
    fn surprise_words_push_acetylcholine_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Acetylcholine);
        state.update("えっ、うそでしょ", 0, false);
        assert!(state.level(DriveKey::Acetylcholine) > baseline);
    }

    /// Regression test for the exclamation-mark bug: `URGENT` used to include "！"/"!", so any
    /// exclaimed line — even a purely happy one with no urgent or conflict word at all — fired
    /// `urgent` and pushed adrenaline up, reading as angry. A happy line with an exclamation mark
    /// must not move adrenaline any more than the same line without one.
    #[test]
    fn an_exclaimed_happy_line_does_not_raise_adrenaline() {
        let mut with_bang = AffectState::default();
        let mut without_bang = AffectState::default();
        with_bang.update("今日の発表、すごく褒められたんだ!", 0, false);
        without_bang.update("今日の発表、すごく褒められたんだ", 0, false);
        let baseline = DriveKey::Adrenaline.info().base;
        assert_eq!(with_bang.level(DriveKey::Adrenaline), baseline);
        assert_eq!(with_bang.level(DriveKey::Adrenaline), without_bang.level(DriveKey::Adrenaline));
    }

    /// Regression test for sad's near-dead recall (6.7% against the eval set): a plain pain
    /// statement, with no conflict or urgency, must visibly move every drive sad's votes in
    /// vrm-emotion.ts's `PRIMARY_VOTES` read from — cortisol and dynorphin up (shared with
    /// angry's trigger), substance_p up, and serotonin down (previously only conflict pulled
    /// serotonin down, leaving a purely sad turn's serotonin untouched).
    #[test]
    fn a_pain_statement_moves_every_sad_voting_drive() {
        let mut state = AffectState::default();
        let cortisol_base = state.level(DriveKey::Cortisol);
        let dynorphin_base = state.level(DriveKey::Dynorphin);
        let substance_p_base = state.level(DriveKey::SubstanceP);
        let serotonin_base = state.level(DriveKey::Serotonin);
        state.update("寂しくて悲しい", 0, false);
        assert!(state.level(DriveKey::Cortisol) > cortisol_base);
        assert!(state.level(DriveKey::Dynorphin) > dynorphin_base);
        assert!(state.level(DriveKey::SubstanceP) > substance_p_base);
        assert!(state.level(DriveKey::Serotonin) < serotonin_base);
    }

    /// A pain-only (no conflict) turn should barely move cck: cck only ever votes for `angry` in
    /// vrm-emotion.ts, never for `sad`, so a plain sad statement raising it at full strength (as
    /// it used to, sharing `conflict`'s trigger) meant a purely sad turn could clear cck's low
    /// angry floor and read as angry. A conflict turn should still move it clearly.
    #[test]
    fn a_pain_only_statement_barely_moves_cck_but_conflict_does() {
        let mut pain_only = AffectState::default();
        let mut conflict = AffectState::default();
        let base = DriveKey::Cck.info().base;
        pain_only.update("寂しくて悲しい", 0, false);
        conflict.update("うるさい、最悪だよ", 0, false);
        let pain_only_deviation = pain_only.level(DriveKey::Cck) - base;
        let conflict_deviation = conflict.level(DriveKey::Cck) - base;
        assert!(pain_only_deviation > 0.0);
        assert!(conflict_deviation > pain_only_deviation * 2.0);
    }

    /// Regression test for the opioid-drive pass (see the module doc comment): a comforting,
    /// reassuring utterance — no CALM or RECOVER word, only COMFORT ("大丈夫" here doubles as a
    /// COMFORT entry) — should push enkephalin clearly above its baseline. Before this pass
    /// enkephalin's only trigger was `comfort` at a weight (0.13) that normalized to well under
    /// vrm-emotion.ts's DEVIATION_FLOOR, so it fired on only 9/288 eval-set turns; this checks
    /// the raised weight (0.18) actually moves the level, not just that it's wired up.
    #[test]
    fn a_comforting_utterance_pushes_enkephalin_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Enkephalin);
        state.update("大丈夫だよ、ゆっくりでいいから", 0, false);
        assert!(state.level(DriveKey::Enkephalin) > baseline);
    }

    /// Regression test for anandamide's dead vote: the old trigger was a logical AND of
    /// `positive && calm`, two conditions that essentially never co-occur in one utterance
    /// (measured: fired 1/288 times against the eval set). A plain positive utterance alone,
    /// with no calm word at all, must now move anandamide — the whole point of splitting the AND
    /// into independent additive terms was that any one of them (here, `positive`) can start
    /// moving the drive without needing the others.
    #[test]
    fn a_positive_utterance_alone_pushes_anandamide_above_baseline() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Anandamide);
        state.update("助かった、本当にありがとう", 0, false);
        assert!(state.level(DriveKey::Anandamide) > baseline);
    }

    /// Companion to the previous test: a calm-only utterance (no positive/humor/recover word)
    /// must *not* move anandamide on its own — `calm` was deliberately dropped from anandamide's
    /// trigger (see its delta comment) because letting a calm word feed `happy` as well as
    /// `relaxed` double-counted the same word across two competing vrm-emotion.ts expressions,
    /// measured to cost both neutral and relaxed recall. This pins that decision down: if `calm`
    /// is ever added back here, this test should fail and prompt re-reading why it was removed.
    #[test]
    fn a_calm_only_utterance_does_not_move_anandamide() {
        let mut state = AffectState::default();
        let baseline = state.level(DriveKey::Anandamide);
        state.update("静かでのんびりした時間だった", 0, false);
        assert_eq!(state.level(DriveKey::Anandamide), baseline);
    }

    /// The web UI contract (`docs/` WS `affect` frame) hard-codes 22 keys in
    /// exactly this order; a drift here is a breaking wire-format change.
    const CONTRACT_DRIVE_KEYS: [&str; DRIVE_COUNT] = [
        "dopamine",
        "serotonin",
        "oxytocin",
        "endorphin",
        "cortisol",
        "noradrenaline",
        "adrenaline",
        "acetylcholine",
        "glutamate",
        "gaba",
        "glycine",
        "melatonin",
        "orexin",
        "histamine",
        "dynorphin",
        "dhea",
        "enkephalin",
        "anandamide",
        "substance_p",
        "npy",
        "cck",
        "bdnf",
    ];

    #[test]
    fn snapshot_has_all_22_drives_in_all_drives_order_with_contract_keys() {
        let state = AffectState::default();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.drives.len(), DRIVE_COUNT);

        let keys: Vec<&str> = snapshot.drives.iter().map(|d| d.key).collect();
        assert_eq!(keys, CONTRACT_DRIVE_KEYS);

        // Order also matches ALL_DRIVES's own declaration order directly.
        for (snap, key) in snapshot.drives.iter().zip(ALL_DRIVES.iter()) {
            assert_eq!(snap.key, key.as_str());
            assert_eq!(snap.base, key.info().base);
            assert_eq!(snap.level, state.level(*key));
        }
    }

    #[test]
    fn snapshot_reflects_familiarity_closing_and_invite_caution() {
        let mut state = AffectState::default();
        assert_eq!(state.snapshot().familiarity, 0.0);
        assert!(!state.snapshot().closing);
        assert!(!state.snapshot().invite_caution);

        state.update("一緒に行こう", 0, false);
        let snapshot = state.snapshot();
        assert!(snapshot.invite_caution);
        assert!(!snapshot.closing);
        assert_eq!(snapshot.familiarity, state.familiarity());

        state.update("じゃあね", 1, false);
        assert!(state.snapshot().closing);
    }

    // --- PartnerAffect --------------------------------------------------

    /// Enough warm turns to move familiarity clearly off zero, so the tests
    /// below can tell "carried over" from "started fresh" without asserting
    /// on the exact tuned deltas.
    fn warm_up(partners: &mut PartnerAffect) {
        for _ in 0..3 {
            partners.update("ありがとう、嬉しいです", Instant::now());
        }
    }

    #[test]
    fn unnamed_speaker_keeps_the_current_partner() {
        let mut partners = PartnerAffect::default();
        assert!(partners.observe(Some("太郎")).is_some());
        warm_up(&mut partners);
        let familiarity = partners.state().familiarity();

        // The web UI's chat box sends no speaker, and a blank one is the same
        // non-signal — neither may read as "the partner changed".
        assert!(partners.observe(None).is_none());
        assert!(partners.observe(Some("   ")).is_none());
        assert_eq!(partners.snapshot().partner.as_deref(), Some("太郎"));
        assert_eq!(partners.state().familiarity(), familiarity);
    }

    #[test]
    fn a_new_partner_starts_from_a_fresh_state() {
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        warm_up(&mut partners);
        assert!(partners.state().familiarity() > 0.0);

        let switch = partners.observe(Some("花子")).expect("switch detected");
        assert_eq!(switch.from.as_deref(), Some("太郎"));
        assert_eq!(switch.to, "花子");
        assert!(!switch.returning);
        // The whole point: warmth earned with 太郎 must not transfer.
        assert_eq!(partners.state().familiarity(), 0.0);
        assert!(!partners.snapshot().partner_known);
    }

    #[test]
    fn returning_partner_resumes_their_own_state() {
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        warm_up(&mut partners);
        let taro_familiarity = partners.state().familiarity();

        partners.observe(Some("花子"));
        partners.update("こんにちは", Instant::now());

        let switch = partners.observe(Some("太郎")).expect("switch detected");
        assert!(switch.returning);
        assert!(partners.snapshot().partner_known);
        assert_eq!(partners.state().familiarity(), taro_familiarity);
    }

    #[test]
    fn honorifics_and_spacing_do_not_look_like_a_different_partner() {
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        assert!(partners.observe(Some("太郎さん")).is_none());
        assert!(partners.observe(Some("　太郎　")).is_none());
    }

    #[test]
    fn switched_flag_marks_only_the_turn_the_partner_changed() {
        let mut partners = PartnerAffect::default();
        assert!(!partners.snapshot().partner_switched);

        partners.observe(Some("太郎"));
        assert!(partners.snapshot().partner_switched);

        // Same partner again — no longer a switch.
        partners.observe(Some("太郎"));
        assert!(!partners.snapshot().partner_switched);
    }

    #[test]
    fn tracking_is_bounded_and_drops_the_least_recent_partner() {
        let mut partners = PartnerAffect::default();
        partners.observe(Some("最初"));
        warm_up(&mut partners);

        // Push past the cap with distinct names.
        for i in 0..MAX_TRACKED_PARTNERS {
            partners.observe(Some(&format!("人{i}")));
            partners.update("こんにちは", Instant::now());
        }

        // 最初 has been evicted, so coming back reads as a first meeting.
        let switch = partners.observe(Some("最初")).expect("switch detected");
        assert!(!switch.returning);
        assert_eq!(partners.state().familiarity(), 0.0);
    }

    const ABSENCE: Duration = Duration::from_secs(300);

    #[test]
    fn silence_past_the_timeout_reads_as_the_partner_having_left() {
        let start = Instant::now();
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        partners.update("一緒に行こう", start);
        assert!(partners.snapshot().invite_caution);
        assert!(!partners.snapshot().partner_away);

        // Still within the timeout — nothing has happened yet.
        assert!(partners
            .check_absence(start + Duration::from_secs(299), ABSENCE)
            .is_none());
        assert!(!partners.snapshot().partner_away);

        let absence = partners
            .check_absence(start + Duration::from_secs(301), ABSENCE)
            .expect("absence detected");
        assert_eq!(absence.who.as_deref(), Some("太郎"));

        let snapshot = partners.snapshot();
        assert!(snapshot.partner_away);
        // The conversation wound down: the state machine cleared and the
        // drives went back to rest...
        assert!(!snapshot.invite_caution);
        assert!(!snapshot.closing);
        assert_eq!(
            partners.state().level(DriveKey::Cortisol),
            DriveKey::Cortisol.info().base
        );
        // ...but the NPC still knows who 太郎 is, and still says so.
        assert_eq!(snapshot.partner.as_deref(), Some("太郎"));
    }

    #[test]
    fn familiarity_survives_the_partner_stepping_away() {
        let start = Instant::now();
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        for _ in 0..3 {
            partners.update("ありがとう、嬉しいです", start);
        }
        let familiarity = partners.state().familiarity();
        assert!(familiarity > 0.0);

        partners.check_absence(start + Duration::from_secs(600), ABSENCE);
        assert_eq!(partners.state().familiarity(), familiarity);
    }

    #[test]
    fn one_silence_is_reported_once_however_often_it_is_polled() {
        let start = Instant::now();
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        partners.update("こんにちは", start);

        assert!(partners.check_absence(start + Duration::from_secs(400), ABSENCE).is_some());
        // A poll runs every few seconds forever; it must not keep firing.
        assert!(partners.check_absence(start + Duration::from_secs(500), ABSENCE).is_none());
        assert!(partners.check_absence(start + Duration::from_secs(9000), ABSENCE).is_none());
    }

    #[test]
    fn speaking_again_clears_the_away_state() {
        let start = Instant::now();
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        partners.update("こんにちは", start);
        partners.check_absence(start + Duration::from_secs(400), ABSENCE);
        assert!(partners.snapshot().partner_away);

        let resumed = start + Duration::from_secs(500);
        partners.observe(Some("太郎"));
        partners.update("ただいま", resumed);
        assert!(!partners.snapshot().partner_away);
        // And the silence clock restarted from the new turn.
        assert!(partners.check_absence(resumed + Duration::from_secs(100), ABSENCE).is_none());
    }

    #[test]
    fn absence_check_is_disabled_by_a_zero_timeout() {
        let start = Instant::now();
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        partners.update("こんにちは", start);
        assert!(partners
            .check_absence(start + Duration::from_secs(86_400), Duration::ZERO)
            .is_none());
        assert!(!partners.snapshot().partner_away);
    }

    #[test]
    fn nothing_is_absent_before_anyone_has_spoken() {
        let mut partners = PartnerAffect::default();
        assert!(partners
            .check_absence(Instant::now() + Duration::from_secs(9000), ABSENCE)
            .is_none());
    }

    #[test]
    fn bond_ramp_counts_turns_per_partner_not_globally() {
        // own_turns feeds the slow oxytocin ramp. A partner met after a long
        // conversation with someone else must start that ramp at zero.
        let mut partners = PartnerAffect::default();
        partners.observe(Some("太郎"));
        for _ in 0..10 {
            partners.update("ありがとう、嬉しいです", Instant::now());
        }
        let taro_oxytocin = partners.state().level(DriveKey::Oxytocin);

        partners.observe(Some("花子"));
        partners.update("ありがとう、嬉しいです", Instant::now());
        assert!(partners.state().level(DriveKey::Oxytocin) < taro_oxytocin);
    }

    /// Regression test for the unbounded-integrator saturation bug: 30
    /// turns of an unremarkable 16+-character declarative sentence used to
    /// pin dopamine, oxytocin, acetylcholine and orexin at exactly `1.0`
    /// (and, with a question mixed in, noradrenaline/histamine too). The
    /// bounded integrator's fixed point is guaranteed to sit strictly
    /// inside `(0, 1)`, so none of the 22 drives should ever reach either
    /// rail from ordinary conversation alone.
    #[test]
    fn ordinary_conversation_never_pins_a_drive_to_its_ceiling() {
        let mut state = AffectState::default();
        let plain = "今日はとても天気が良くて気持ちいいですね";
        for turn in 0..30 {
            state.update(plain, turn, false);
        }
        for key in ALL_DRIVES {
            let level = state.level(key);
            assert!(level < 1.0, "{key:?} pinned to the ceiling: {level}");
            assert!(level > 0.0, "{key:?} pinned to the floor: {level}");
        }
    }

    /// Being at a high plateau is not the same as being clamped: unlike the
    /// old unbounded integrator's flat `1.0`, the bounded integrator's
    /// fixed point is an ordinary interior value that still moves when the
    /// input changes. After the same 30-turn plateau as the test above,
    /// one short reply (which flips dopamine's delta from +0.18 to -0.12,
    /// the "terse" branch) should measurably pull dopamine back down.
    #[test]
    fn a_saturated_drive_still_responds_to_a_stronger_stimulus() {
        let mut state = AffectState::default();
        let plain = "今日はとても天気が良くて気持ちいいですね";
        for turn in 0..30 {
            state.update(plain, turn, false);
        }
        let plateaued = state.level(DriveKey::Dopamine);
        assert!(plateaued > 0.6 && plateaued < 1.0, "unexpected plateau: {plateaued}");

        state.update("うん", 30, false);
        assert!(
            state.level(DriveKey::Dopamine) < plateaued,
            "dopamine did not respond: still {}",
            state.level(DriveKey::Dopamine)
        );
    }

    /// Regression test for `to_prompt` going stale: once the old unbounded
    /// integrator pinned dopamine/oxytocin/acetylcholine/orexin to `1.0`,
    /// the top-3-by-deviation salience in `to_prompt` locked onto the same
    /// three drives and printed the identical string every turn for the
    /// rest of the conversation. With the bounded integrator the drives
    /// keep moving (even if only within a plateau), so the rendered
    /// internal-state text should show at least some variation across a
    /// long conversation rather than freezing after the first few turns.
    #[test]
    fn internal_state_prompt_keeps_changing_over_a_long_conversation() {
        let mut state = AffectState::default();
        let plain = "今日はとても天気が良くて気持ちいいですね";
        let mut prompts = Vec::new();
        for turn in 0..30 {
            state.update(plain, turn, false);
            prompts.push(state.to_prompt());
        }
        let distinct: HashSet<&String> = prompts.iter().collect();
        assert!(
            distinct.len() > 1,
            "to_prompt printed the exact same text for all 30 turns"
        );
        assert_ne!(prompts.first(), prompts.last());
    }

    #[test]
    fn minimal_ack_after_closing_stays_silent() {
        let mut state = AffectState::default();
        state.update("じゃあね", 5, false);
        assert!(state.is_closing());
        // First closing turn: still the farewell reply, not the "……" fallback.
        state.update("うん", 6, false);
        assert!(state.is_closing());
        assert_eq!(state.forced_closure_reply("うん"), Some("……".to_string()));
    }
}
