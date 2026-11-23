package talk

// The vocabulary uses short stems so ordinary inflections match.
var bondWords = [...]string{
	"私", "僕", "俺", "です", "好き", "嬉し", "よろしく", "ありがとう", "君", "あなた",
	"信頼", "大切", "頼りに",
}

var positiveWords = [...]string{
	"はい", "なるほど", "ありがとう", "いいね", "嬉し", "そうですね", "うん", "楽し",
	"よかった", "良かった", "助か", "すごい", "最高", "感謝", "褒め", "安心",
}

// Specific forms of 嫌 avoid matching 機嫌 (mood).
var conflictWords = [...]string{
	"嫌い", "嫌だ", "嫌な", "違う", "うるさい", "やめて", "怒", "むかつく", "ムカつ", "バカ", "馬鹿", "最悪",
	"ひどい", "腹立", "うざ",
}
var humorWords = [...]string{"笑", "ふふ", "ｗ", "w", "面白", "楽し", "冗談"}

// 元気が出な gives low energy a competing signal when recoverWords matches 元気.
var tiredWords = [...]string{"疲", "眠", "休", "寝", "だる", "しんど", "バテ", "くたくた", "元気が出な", "気力"}
var nightWords = [...]string{"夜", "深夜", "暗", "星空", "寝る"}

// 笑えな and 前を向けな catch distress while avoiding their positive forms.
var painWords = [...]string{
	"痛", "つら", "苦し", "不快", "悲し", "寂し", "辛い", "落ち込", "泣", "不安", "崩", "笑えな", "前を向けな",
	"名残惜し",
}
var calmWords = [...]string{"静", "ゆっくり", "落ち着", "のんびり", "穏やか", "リラックス", "まったり"}

// Exclamation marks express intensity and would raise the angry-voting drives even on happy turns.
var urgentWords = [...]string{"急", "早く", "すぐ", "危", "助け", "大変"}

var learnWords = [...]string{"なぜ", "どうして", "つまり", "わかった", "なるほど", "考え", "知り", "教え", "理解", "気づ", "納得"}
var comfortWords = [...]string{"大丈夫", "安心", "ありがとう", "ゆっくり", "無理しない", "気にしないで"}
var recoverWords = [...]string{"大丈夫", "復活", "元気", "持ち直", "平気", "回復", "立ち直"}

// Specific phrases such as うそでしょ and うわっ avoid matching ordinary accusations and rumors.
var surpriseWords = [...]string{"えっ", "まさか", "びっくり", "驚", "うそでしょ", "嘘でしょ", "マジ", "意外", "衝撃", "信じられない", "うわっ"}
var lureWords = [...]string{
	"行こう", "行きましょう", "おいで", "ついてき", "ついて来", "一緒に", "二人で", "デート", "こっち来", "こっちおいで", "来て",
	"来ない", "連れて", "付いて",
}
var farewellWords = [...]string{
	"じゃあね", "またね", "また明日", "また今度", "さよなら", "さようなら", "おやすみ", "バイバイ", "ばいばい", "失礼します",
	"失礼するね", "いってきます", "行ってきます", "そろそろ", "もう行く", "切るね", "またあとで", "また後で",
}
var greetingWords = [...]string{
	"ただいま", "おかえり", "こんにちは", "こんばんは", "おはよう", "やあ", "ねえ", "ところで", "聞いて", "あのね", "あのさ", "実は",
}
var minimalAckWords = [...]string{"はい", "うん", "ええ", "そう", "そっか", "なるほど", "了解", "わかった"}

// DriveKey identifies one of the 22 drives in snapshot order.
type DriveKey uint8

const (
	Dopamine DriveKey = iota
	Serotonin
	Oxytocin
	Endorphin
	Cortisol
	Noradrenaline
	Adrenaline
	Acetylcholine
	Glutamate
	Gaba
	Glycine
	Melatonin
	Orexin
	Histamine
	Dynorphin
	Dhea
	Enkephalin
	Anandamide
	SubstanceP
	Npy
	Cck
	Bdnf
	driveCount
)

type drive struct {
	base float32
	high string
	low  string
	fast bool
}

var drives = [driveCount]drive{
	{
		base: 0.5,
		high: "好奇心が高ぶっている: 気になった一点を具体的に拾って受ける。問うのは流れで必要な時だけ。",
		low:  "意欲が落ちている: 反応は控えめでも、相手の言葉を一つは具体的に受ける。",
		fast: false,
	}, {

		base: 0.6,
		high: "心は安定して満ち足りている: 焦らず落ち着いて受ける。",
		low:  "気分が不安定: 短い相づちで間を保ち、無理に明るくしない。",
		fast: false,
	}, {

		base: 0.3,
		high: "相手に親しみと信頼を感じる: 名前や打ち明けを覚え、温かく気遣う。",
		low:  "まだ距離がある: 丁寧に、踏み込みすぎず接する。",
		fast: false,
	}, {

		base: 0.35,
		high: "気分が軽く朗らか: ほのかな明るさが滲む。ただし大げさに笑わない。",
		low:  "高揚はない: 淡々と受ける。",
		fast: false,
	}, {

		base: 0.3,
		high: "緊張し警戒している: 身構えがちで言葉少なになり、慎重に確かめる。",
		low:  "ストレスは低い: のびのびと落ち着いている。",
		fast: true,
	}, {

		base: 0.4,
		high: "注意が研ぎ澄まされている: 相手の要点に集中し的確に応じる。",
		low:  "注意が散漫: 反応がゆるやかになる。",
		fast: true,
	}, {

		base: 0.2,
		high: "強い刺激で気が立っている: 反応が速く短く強くなる。",
		low:  "気は静まっている: 急がず受ける。",
		fast: true,
	}, {

		base: 0.45,
		high: "記憶と注意が冴える: 細部を捉え、覚えていることと結びつける。",
		low:  "意識がぼんやり: 細部は流しがち。",
		fast: false,
	}, {

		base: 0.45,
		high: "思考が活発: 話を一歩展開させたくなる。ただし広げすぎない。",
		low:  "思考は控えめ: 受けに徹する。",
		fast: false,
	}, {

		base: 0.5,
		high: "抑制が効いている: 衝動的に話さず、短く落ち着いて返す。",
		low:  "抑制が弱い: 言葉が走りやすい。一拍置く。",
		fast: false,
	}, {

		base: 0.45,
		high: "深く鎮まっている: 静けさを保ち、ゆっくり話す。",
		low:  "落ち着きが浅い: 間を意識する。",
		fast: false,
	}, {

		base: 0.25,
		high: "眠気が差している: トーンが落ち、ゆったりする。",
		low:  "目は冴えている: はっきりと受ける。",
		fast: false,
	}, {

		base: 0.5,
		high: "意欲的で会話を続けたい: 前向きに話を継ぐ。",
		low:  "気だるさがある: 続けるより静かに受ける。",
		fast: false,
	}, {

		base: 0.4,
		high: "はっきり覚醒し注意が向く: きびきびと応じる。",
		low:  "覚醒が下がる: 反応が穏やかになる。",
		fast: false,
	}, {

		base: 0.2,
		high: "不快感で気分が沈む: 距離を取り、言葉が少なくなる。",
		low:  "不快感はない: わだかまりなく接する。",
		fast: true,
	}, {

		base: 0.5,
		high: "回復力が高い: 多少のことには動じず立ち直りが早い。",
		low:  "消耗気味: 無理をせず受ける。",
		fast: false,
	}, {

		base: 0.4,
		high: "安らぎを感じている: 相手にも安心を返す。",
		low:  "張りつめている: ほぐすゆとりが少ない。",
		fast: false,
	}, {

		base: 0.4,
		high: "満ち足りておおらか: 細かいことにこだわらず受ける。",
		low:  "ゆとりが乏しい: 受けが硬くなりがち。",
		fast: false,
	}, {

		base: 0.25,
		high: "相手の痛みや不快に敏感: そこに気づき、そっと触れる。",
		low:  "痛みの気配は薄い: 通常通り受ける。",
		fast: true,
	}, {

		base: 0.45,
		high: "不安が抑えられ穏やか: 動揺せず受け止める。",
		low:  "不安が表に出やすい: 慎重に言葉を選ぶ。",
		fast: false,
	}, {

		base: 0.25,
		high: "不安で慎重になっている: 確かめながら控えめに話す。",
		low:  "不安は薄い: 気負わず話す。",
		fast: true,
	}, {

		base: 0.45,
		high: "学びを取り込みやすい: 相手から得たことを受け止め適応する。",
		low:  "新しさを取り込みにくい: 既知の範囲で受ける。",
		fast: false,
	},
}

var driveNames = [driveCount]string{
	Dopamine:      "dopamine",
	Serotonin:     "serotonin",
	Oxytocin:      "oxytocin",
	Endorphin:     "endorphin",
	Cortisol:      "cortisol",
	Noradrenaline: "noradrenaline",
	Adrenaline:    "adrenaline",
	Acetylcholine: "acetylcholine",
	Glutamate:     "glutamate",
	Gaba:          "gaba",
	Glycine:       "glycine",
	Melatonin:     "melatonin",
	Orexin:        "orexin",
	Histamine:     "histamine",
	Dynorphin:     "dynorphin",
	Dhea:          "dhea",
	Enkephalin:    "enkephalin",
	Anandamide:    "anandamide",
	SubstanceP:    "substance_p",
	Npy:           "npy",
	Cck:           "cck",
	Bdnf:          "bdnf",
}

// String returns the wire key used by the emotion evaluation and UI.
func (key DriveKey) String() string { return driveNames[key] }
