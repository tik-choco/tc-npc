package translate

import "testing"

func TestDetectLanguage(t *testing.T) {
	for _, tc := range []struct {
		text string
		want string
	}{
		{"こんにちは", langJapanese},
		{"カタカナ", langJapanese},
		{"OKです、了解", langJapanese}, // Kana wins in mixed text.
		{"hello there", langEnglish},
		{"OK", ""},
		{"Hi", ""},
		{"今日会議", ""}, // Han without kana is ambiguous.
		{"你好世界", ""},
		{"123 !!!", ""},
		{"", ""},
		{"abc\u3040", langJapanese}, // The kana test covers the whole block.
		{"abc\u30ff", langJapanese},
		{"abc\u3400", ""},
		{"abc\u9fff", ""},
		{"abc\ufaff", ""},
		{"abcｶ", langEnglish}, // Half-width kana and Han Extension B fall outside the ranges.
		{"abc\U00020000", langEnglish},
		{"éçö", ""},
	} {
		if got := detectLanguage(tc.text); got != tc.want {
			t.Errorf("detectLanguage(%q) = %q, want %q", tc.text, got, tc.want)
		}
	}
}
