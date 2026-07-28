//! Emotion-drive model and closing-turn state machine, ported from the
//! reference TypeScript implementation `tc-assistant2/src/conversation.ts`
//! (`ConversationAffectState`, lines 170-327, plus the vocabulary constants
//! at the top of that file). Memory and persona handling live elsewhere in
//! this workspace (`npc-memory`, persona prompts) and are *not* touched
//! here — this module is purely the internal "drive" state and the
//! farewell/closing state machine layered on top of it.
//!
//! Numeric constants (bases, deltas, decay pulls, thresholds) are copied
//! verbatim from the TypeScript original; see the doc comments on
//! individual items for the corresponding line ranges.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;

// ---------------------------------------------------------------------
// vocabulary constants (conversation.ts lines 42-57)
// ---------------------------------------------------------------------

const BOND: &[&str] = &["私", "僕", "俺", "です", "好き", "嬉しい", "よろしく", "ありがとう", "君", "あなた"];
const POSITIVE: &[&str] = &["はい", "なるほど", "ありがとう", "いいね", "嬉しい", "そうですね", "うん", "楽し"];
const CONFLICT: &[&str] = &["嫌", "違う", "うるさい", "やめて", "怒", "むかつく", "バカ", "最悪"];
const HUMOR: &[&str] = &["笑", "ふふ", "ｗ", "w", "面白", "楽し", "冗談"];
const TIRED: &[&str] = &["疲", "眠", "休", "寝", "だる", "しんど"];
const NIGHT: &[&str] = &["夜", "深夜", "暗", "星空", "寝る"];
const PAIN: &[&str] = &["痛", "つら", "苦し", "不快", "悲し", "寂し"];
const CALM: &[&str] = &["静", "ゆっくり", "落ち着", "のんびり", "穏やか"];
const URGENT: &[&str] = &["急", "早く", "すぐ", "危", "！", "!", "助け", "大変"];
const LEARN: &[&str] = &["なぜ", "どうして", "つまり", "わかった", "なるほど", "考え", "知り", "教え"];
const COMFORT: &[&str] = &["大丈夫", "安心", "ありがとう", "ゆっくり", "無理しない"];
const RECOVER: &[&str] = &["大丈夫", "復活", "元気", "持ち直", "平気"];
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

        let mut delta = [0.0f32; DRIVE_COUNT];

        if partner_known {
            self.familiarity = self.familiarity.max(0.7);
        }

        delta[Dopamine as usize] += if question || novelty {
            0.18
        } else if terse {
            -0.12
        } else {
            -0.03
        };
        delta[Serotonin as usize] += b(positive, 0.06) - b(conflict, 0.15);
        delta[Oxytocin as usize] += b(bond, 0.15) + 0.04 * (own_turns.min(4) as f32) / 4.0;
        delta[Endorphin as usize] += b(humor, 0.18) + b(positive, 0.05);
        delta[Cortisol as usize] += b(conflict || urgent || pain, 0.18) - b(positive || calm, 0.06);
        delta[Noradrenaline as usize] += b(question || urgent, 0.16) - b(terse, 0.06);
        delta[Adrenaline as usize] += b(urgent || (conflict && question), 0.22);
        delta[Acetylcholine as usize] += b(learn || char_count >= 16, 0.13);
        delta[Glutamate as usize] += b(learn, 0.13);
        delta[Gaba as usize] += b(calm, 0.12) - b(urgent || conflict, 0.1);
        delta[Glycine as usize] += b(calm || night, 0.13);
        delta[Melatonin as usize] += b(night || tired, 0.2) - b(urgent, 0.08);
        delta[Orexin as usize] += b(question || novelty || positive, 0.12) - b(tired || night, 0.16);
        delta[Histamine as usize] += b(question || urgent, 0.12) - b(tired || night, 0.1);
        delta[Dynorphin as usize] += b(pain || conflict, 0.15);
        delta[Dhea as usize] += b(recover || positive, 0.12);
        delta[Enkephalin as usize] += b(comfort, 0.13);
        delta[Anandamide as usize] += b(positive && calm, 0.12) + b(humor, 0.06);
        delta[SubstanceP as usize] += b(pain, 0.16);
        delta[Npy as usize] += b(calm, 0.1) + b(recover, 0.06);
        delta[Cck as usize] += b(conflict || pain, 0.13);
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
            let decayed = self.levels[i] + (info.base - self.levels[i]) * pull;
            self.levels[i] = clamp(decayed + delta[i]);
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
