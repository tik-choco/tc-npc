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
        }
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
