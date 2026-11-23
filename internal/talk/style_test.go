package talk

import (
	"crypto/sha256"
	"fmt"
	"strings"
	"testing"
)

func TestSanitizeReply(t *testing.T) {
	for _, tc := range []struct{ name, input, want string }{
		{"opening direction", "（ふっと視線を緩め、あなたの本音に寄り添うように静かに口角を上げる）\n\n……そうですね。", "……そうですね。"},
		{"trailing direction", "はい。（微笑む）", "はい。"},
		{"halfwidth direction", "はい。(小さく頷く)", "はい。"},
		{"halfwidth prose", "v2(beta)を試します。", "v2(beta)を試します。"},
		{"halfwidth time", "(9時)に始めます。", "(9時)に始めます。"},
		{"roleplay asterisks", "*ため息* わかりました。", "わかりました。"},
		{"emphasis", "**本当に**そう思います。", "本当にそう思います。"},
		{"blank lines", "\n\nこんにちは。\n\nお元気ですか。\n", "こんにちは。\nお元気ですか。"},
		{"trimmed edges", "  はい。  ", "はい。"},
		{"fullwidth spaces", "はい。　　そうですね。", "はい。 そうですね。"},
		{"list markers", "- はい。\n- そうですね。", "はい。\nそうですね。"},
		{"Unicode numbered list marker", "\U00010D41. はい。", "はい。"},
		{"heading", "## 返答\nはい。", "返答\nはい。"},
		{"fences", "```\nはい。\n```", "はい。"},
		{"direction only", "\n（微笑む）\n", ""},
		{"ordinary speech", "ほうじ茶、昨日も飲んでましたね。", "ほうじ茶、昨日も飲んでましたね。"},
		{"Japanese quotation", "「はい」と答えました。", "「はい」と答えました。"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := SanitizeReply(tc.input); got != tc.want {
				t.Errorf("SanitizeReply(%q) = %q, want %q", tc.input, got, tc.want)
			}
		})
	}
}

func TestIsSilence(t *testing.T) {
	for _, input := range []string{SilenceSentinel, " <silence> ", "", "……", "...", "。"} {
		if !IsSilence(input) {
			t.Errorf("%q should be treated as silence", input)
		}
	}
	for _, input := range []string{"はい。", "……そうですね。", "ok", "\u0363", "\U00011380"} {
		if IsSilence(input) {
			t.Errorf("%q should be spoken", input)
		}
	}
	for _, input := range []string{"\n（微笑む）\n", "<silence>", "（少し黙る）"} {
		if !IsSilence(SanitizeReply(input)) {
			t.Errorf("%q should be treated as silence after sanitizing", input)
		}
	}
}

func TestSpokenReplyRules(t *testing.T) {
	for _, fragment := range []string{SilenceSentinel, "毎回必ず応答しなくてよい", "ト書き", "改行しない", "Markdown"} {
		if !strings.Contains(SpokenReplyRules, fragment) {
			t.Errorf("rules missing %q", fragment)
		}
	}
	const rustSHA256 = "0aea6f75e8bf6bdebc96833687e69a26eb572b2c3a49cf683e2c29245f1e53d5"
	if got := fmt.Sprintf("%x", sha256.Sum256([]byte(SpokenReplyRules))); got != rustSHA256 {
		t.Errorf("rules SHA-256 = %s, want Rust SHA-256 %s", got, rustSHA256)
	}
}
