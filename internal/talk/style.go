package talk

import (
	"regexp"
	"strings"
	"unicode"
)

// SilenceSentinel is the tag the model emits to end a turn without speaking.
const SilenceSentinel = "<silence>"

// IsSilence recognizes the tag and replies with nothing pronounceable left.
func IsSilence(text string) bool {
	if strings.Contains(text, SilenceSentinel) {
		return true
	}
	for _, r := range text {
		if unicode.IsLetter(r) || unicode.IsNumber(r) || unicode.Is(unicode.Other_Alphabetic, r) {
			return false
		}
	}
	return !additionalAlphanumeric.MatchString(text)
}

const unicodeSpace = `[\t\n\x{000B}\f\r\x{0085}\p{Z}]`

// Rust's regex tables recognize these decimal digits beyond Go's Unicode 15.
const unicodeDigit = `[\p{Nd}\x{10D40}-\x{10D49}\x{116D0}-\x{116E3}\x{11BF0}-\x{11BF9}` +
	`\x{16130}-\x{16139}\x{16D70}-\x{16D79}\x{1CCF0}-\x{1CCF9}\x{1E5F1}-\x{1E5FA}]`

var (
	fullwidthParen = regexp.MustCompile(`（[^（）]*）`)
	halfwidthParen = regexp.MustCompile(`\(([^()\n]*)\)`)
	bold           = regexp.MustCompile(`\*\*([^*\n]+)\*\*`)
	emphasis       = regexp.MustCompile(`\*[^*\n]*\*`)
	fence          = regexp.MustCompile(`(?m)^` + unicodeSpace + `*` + "```" + `.*$`)
	lineMarker     = regexp.MustCompile(`(?m)^` + unicodeSpace + `*(?:#{1,6}` + unicodeSpace +
		`+|[-*+>]` + unicodeSpace + `+|` + unicodeDigit + `+[.)]` + unicodeSpace + `+)`)
)

// Extra alphanumeric runes recognized by Rust's Unicode 17 char tables.
var additionalAlphanumeric = regexp.MustCompile(
	`[\x{0363}-\x{036F}\x{088F}\x{0897}\x{0C5C}\x{0CDC}\x{1C89}-\x{1C8A}\x{1DD3}-\x{1DE6}` +
		`\x{A7CB}-\x{A7CF}\x{A7D2}\x{A7D4}\x{A7DA}-\x{A7DC}\x{A7F1}\x{105C0}-\x{105F3}` +
		`\x{10940}-\x{10959}\x{10D40}-\x{10D65}\x{10D69}\x{10D6F}-\x{10D85}\x{10EC2}-\x{10EC7}\x{10EFA}-\x{10EFC}` +
		`\x{11380}-\x{11389}\x{1138B}\x{1138E}\x{11390}-\x{113B5}\x{113B7}-\x{113C0}\x{113C2}\x{113C5}` +
		`\x{113C7}-\x{113CA}\x{113CC}-\x{113CD}\x{113D1}\x{113D3}\x{116D0}-\x{116E3}\x{11B60}-\x{11B67}` +
		`\x{11BC0}-\x{11BE0}\x{11BF0}-\x{11BF9}\x{11DB0}-\x{11DDB}\x{11DE0}-\x{11DE9}\x{13460}-\x{143FA}` +
		`\x{16100}-\x{1612E}\x{16130}-\x{16139}\x{16D40}-\x{16D6C}\x{16D70}-\x{16D79}\x{16EA0}-\x{16EB8}` +
		`\x{16EBB}-\x{16ED3}\x{16FF2}-\x{16FF6}\x{187F8}-\x{187FF}\x{18CFF}\x{18D09}-\x{18D1E}\x{18D80}-\x{18DF2}` +
		`\x{1CCF0}-\x{1CCF9}\x{1E5D0}-\x{1E5ED}\x{1E5F0}-\x{1E5FA}\x{1E6C0}-\x{1E6DE}\x{1E6E0}-\x{1E6F5}` +
		`\x{1E6FE}-\x{1E6FF}\x{2B73A}-\x{2B73F}\x{2CEA2}-\x{2CEAD}\x{2EBF0}-\x{2EE5D}\x{323B0}-\x{33479}]`,
)

// SanitizeReply removes stage directions and Markdown before a reply is read
// aloud, published or added to history.
func SanitizeReply(text string) string {
	text = stripMarkup(text)
	var lines []string
	for line := range strings.SplitSeq(text, "\n") {
		line = squeezeSpaces(strings.TrimSpace(line))
		if line != "" {
			lines = append(lines, line)
		}
	}
	return strings.Join(lines, "\n")
}

func stripMarkup(text string) string {
	// Half-width parentheses can contain real content such as (v2) or (9時).
	text = fullwidthParen.ReplaceAllString(text, "")
	text = halfwidthParen.ReplaceAllStringFunc(text, func(span string) string {
		if strings.ContainsFunc(span, hasCJK) && !strings.ContainsFunc(span, asciiAlphanumeric) {
			return ""
		}
		return span
	})

	// Bold encloses speech. Single asterisks enclose stage directions.
	text = bold.ReplaceAllString(text, "$1")
	text = emphasis.ReplaceAllString(text, "")
	text = fence.ReplaceAllString(text, "")
	return lineMarker.ReplaceAllString(text, "")
}

func squeezeSpaces(line string) string {
	var out strings.Builder
	out.Grow(len(line))
	inSpace := false
	for _, r := range line {
		space := r == ' ' || r == '\t' || r == '\u3000'
		if !space {
			out.WriteRune(r)
		} else if !inSpace {
			out.WriteByte(' ')
		}
		inSpace = space
	}
	return strings.TrimSpace(out.String())
}

func hasCJK(r rune) bool {
	return r >= 0x3040 && r <= 0x309f || r >= 0x30a0 && r <= 0x30ff || r >= 0x4e00 && r <= 0x9fff
}

func asciiAlphanumeric(r rune) bool { return asciiLetter(r) || r >= '0' && r <= '9' }
func asciiLetter(r rune) bool       { return r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' }

// SpokenReplyRules is the built-in system prompt for replies read aloud.
const SpokenReplyRules = `これは声に出して交わされる会話です。返答は、そのまま読み上げられる話し言葉だけにしてください。

出力の形:
・出力はあなたが実際に口に出すセリフだけ。地の文・ト書き・状況説明を書かない。
・「（ふっと視線を緩める）」「（微笑む）」「*ため息*」のような、括弧や記号で囲んだ仕草・表情・心情の描写は一切書かない。伝えたい態度は言葉そのもので示すか、書かずに省く。
・見出し、箇条書き、番号付け、Markdown記法（**、##、` + "```" + ` など）、絵文字、顔文字を使わない。
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
・<silence> を返す時は、それだけを出力する。説明や理由を添えない。`
